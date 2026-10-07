//! Portable byte-path operations used by the imported TypeScript resolver.
//! The algorithms follow Bun's `bun_paths` resolve_path semantics (pinned source noted in NOTICE).
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

pub mod platform {
    /// TypeScript's resolver path rules use POSIX separators independent of host OS.
    pub struct Posix;
}

pub mod fs {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Path<'a> {
        pub text: &'a [u8],
        pub pretty: &'a [u8],
    }
    impl<'a> Path<'a> {
        pub const fn init(text: &'a [u8]) -> Self {
            Self { text, pretty: text }
        }
        pub const fn init_with_pretty(text: &'a [u8], pretty: &'a [u8]) -> Self {
            Self { text, pretty }
        }
    }
}

pub mod path_buffer_pool {
    use super::*;
    const BUFFER_LEN: usize = 32 * 1024;
    thread_local! { static POOL: RefCell<Vec<u8>>=RefCell::new(vec![0;BUFFER_LEN]); }
    pub struct Guard(Option<Vec<u8>>);
    impl Deref for Guard {
        type Target = [u8];
        fn deref(&self) -> &[u8] {
            self.0.as_deref().unwrap_or(&[])
        }
    }
    impl DerefMut for Guard {
        fn deref_mut(&mut self) -> &mut [u8] {
            self.0.as_deref_mut().unwrap_or(&mut [])
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            if let Some(mut buf) = self.0.take() {
                if buf.len() < BUFFER_LEN {
                    buf.resize(BUFFER_LEN, 0);
                }
                POOL.with(|p| {
                    let mut slot = p.borrow_mut();
                    if slot.len() < buf.len() {
                        *slot = buf;
                    }
                });
            }
        }
    }
    pub fn get() -> Guard {
        let mut buf = POOL.with(|p| std::mem::take(&mut *p.borrow_mut()));
        if buf.len() < BUFFER_LEN {
            buf.resize(BUFFER_LEN, 0);
        }
        Guard(Some(buf))
    }
}

pub mod resolve_path {
    use super::platform::Posix;
    use std::borrow::Cow;
    pub fn dirname<P>(path: &[u8]) -> &[u8] {
        let mut end = path.len();
        while end > 1 && matches!(path[end - 1], b'/' | b'\\') {
            end -= 1;
        }
        let trimmed = &path[..end];
        let end = trimmed.iter().rposition(|b| *b == b'/' || *b == b'\\');
        match end {
            None => b".",
            Some(0) => &trimmed[..1],
            Some(i) => &trimmed[..i],
        }
    }
    pub fn slashes_to_posix_in_place(path: &mut [u8]) {
        for b in path {
            if *b == b'\\' {
                *b = b'/';
            }
        }
    }
    pub fn windows_volume_name_len(path: &[u8]) -> (usize, usize) {
        if path.len() >= 2 && path[0].is_ascii_alphabetic() && path[1] == b':' {
            (2, 0)
        } else if path.starts_with(b"//") {
            let server = path[2..]
                .iter()
                .position(|b| *b == b'/')
                .map_or(path.len(), |n| n + 2);
            let share = path
                .get(server + 1..)
                .and_then(|p| p.iter().position(|b| *b == b'/'))
                .map_or(path.len(), |n| n + server + 1);
            (share, server)
        } else {
            (0, 0)
        }
    }
    /// Normalize two paths without exposing reusable scratch storage. When copying is explicitly
    /// unnecessary and the paths are identical, return the original borrowed path.
    pub fn relative_normalized<'a, P, const ALWAYS_COPY: bool>(
        from: &'a [u8],
        to: &'a [u8],
    ) -> Cow<'a, [u8]> {
        let from_parts = components(from);
        let to_parts = components(to);
        let mut common = 0;
        while common < from_parts.len()
            && common < to_parts.len()
            && from_parts[common] == to_parts[common]
        {
            common += 1;
        }
        let mut out = Vec::new();
        for _ in common..from_parts.len() {
            if !out.is_empty() {
                out.push(b'/');
            }
            out.extend_from_slice(b"..");
        }
        for part in &to_parts[common..] {
            if !out.is_empty() {
                out.push(b'/');
            }
            out.extend_from_slice(part);
        }
        if out.is_empty() && !ALWAYS_COPY && from == to {
            return Cow::Borrowed(to);
        }
        Cow::Owned(out)
    }
    fn components(path: &[u8]) -> Vec<&[u8]> {
        path.split(|b| *b == b'/' || *b == b'\\')
            .filter(|p| !p.is_empty() && *p != b".")
            .collect()
    }
    #[allow(dead_code)]
    fn _posix(_: Posix) {}
}

pub fn basename_posix(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    let trimmed = &path[..end];
    trimmed.rsplit(|b| *b == b'/').next().unwrap_or(trimmed)
}
pub fn extension(path: &[u8]) -> &[u8] {
    let base = basename_posix(path);
    match base.iter().rposition(|b| *b == b'.') {
        Some(0) | None => b"",
        Some(i) => &base[i..],
    }
}
pub fn join_abs_string_buf(out: &mut Vec<u8>, base: &[u8], part: &[u8]) {
    out.clear();
    if part.starts_with(b"/") {
        out.extend_from_slice(part);
    } else {
        out.extend_from_slice(base);
        if !base.is_empty() && !base.ends_with(b"/") {
            out.push(b'/');
        }
        out.extend_from_slice(part);
    }
}
pub mod string_paths {
    pub fn without_nt_prefix(path: &[u8]) -> &[u8] {
        if !cfg!(windows) {
            return path;
        }
        for prefix in [
            b"\\??\\UNC\\".as_slice(),
            b"\\??\\".as_slice(),
            b"\\\\?\\".as_slice(),
        ] {
            if let Some(rest) = path.strip_prefix(prefix) {
                return rest;
            }
        }
        path
    }
}

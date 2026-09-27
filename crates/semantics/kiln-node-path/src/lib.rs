//! `node:path`'s POSIX algorithms, implemented once for every engine that answers them.
//!
//! Both engines used to carry their own copy: the interpreter scanned with `std::path`, and the
//! native backend hand-wrote the same four decompositions in C. Measured against node across a
//! twenty-one path matrix, the two copies disagreed with node in seven and five places
//! respectively -- and in *different* places, so they also disagreed with each other. That is the
//! failure mode `regress` and `num-bigint` are linked by both engines to avoid: two
//! implementations of one exactly specified thing agree until one of them has a bug, and then
//! nothing can tell you which.
//!
//! `std::path` is deliberately not used here. It knows a great deal about paths, but it knows a
//! *normalising* dialect of them, and node's decompositions are lexical:
//!
//!   * `dirname("a//b")` is `"a/"` to node; `Path::parent` collapses the run and answers `"a"`;
//!   * `dirname("/a/./b")` is `"/a/."` to node; `Path::parent` resolves the `.` away;
//!   * `extname("..")` is `""` but `extname("...")` is `"."`, which no `extension()` call models.
//!
//! So the three scans below are transcribed from node's own implementation, which is the
//! specification here in the way ECMA-262 is elsewhere. `normalize` and `relative` are the two
//! that `std` genuinely does help with, and they are written over iterators accordingly.

#![deny(clippy::panic, clippy::unwrap_used, clippy::indexing_slicing)]

use std::borrow::Cow;

/// Which slice of a path one of `node:path`'s pure decompositions answers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPart {
    /// `dirname`: everything before the last separator.
    Dir,
    /// `basename`: everything after it, optionally with a suffix removed.
    Base,
    /// `extname`: the last dot and what follows, or `""`.
    Ext,
    /// `normalize`: the same path with `.`, `..` and repeated separators resolved.
    Normalize,
}

/// The four pure decompositions of a POSIX path, as node answers them.
#[must_use]
pub fn posix_path_part<'a>(part: PathPart, path: &'a str, suffix: Option<&str>) -> Cow<'a, str> {
    match part {
        PathPart::Dir => dirname(path),
        PathPart::Base => basename(path, suffix),
        PathPart::Ext => extname(path),
        PathPart::Normalize => Cow::Owned(normalize_posix_path(path)),
    }
}

/// `path.dirname`: everything before the last separator, without normalising anything.
///
/// The trailing separators are skipped before the search starts, which is why `dirname("/a/b/")`
/// is `"/a"` and not `"/a/b"`. A run of separators *inside* the path is kept, because node reports
/// the text that precedes the final component rather than a cleaned-up path.
#[must_use]
pub fn dirname(path: &str) -> Cow<'_, str> {
    let bytes = path.as_bytes();
    if bytes.is_empty() {
        return Cow::Borrowed(".");
    }
    let has_root = bytes.first() == Some(&b'/');
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    for index in (1..bytes.len()).rev() {
        if bytes.get(index) == Some(&b'/') {
            if !matched_slash {
                end = Some(index);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None if has_root => Cow::Borrowed("/"),
        None => Cow::Borrowed("."),
        // `//a` is rooted at `//`, which POSIX leaves implementation-defined and node preserves.
        Some(1) if has_root => Cow::Borrowed("//"),
        Some(end) => Cow::Borrowed(path.get(..end).unwrap_or_default()),
    }
}

/// `path.basename`, with node's suffix rule.
///
/// The suffix is only removed when it is a *proper* suffix of the base: `basename("/foo/bar.js",
/// "bar.js")` keeps `"bar.js"`, because removing it would answer with nothing. The one exception
/// is a path that is itself exactly the suffix, which node answers with `""`.
#[must_use]
pub fn basename<'a>(path: &'a str, suffix: Option<&str>) -> Cow<'a, str> {
    let (start, end) = base_span(path);
    let base = path.get(start..end).unwrap_or_default();
    let Some(suffix) = suffix.filter(|suffix| !suffix.is_empty()) else {
        return Cow::Borrowed(base);
    };
    if suffix.len() > path.len() {
        return Cow::Borrowed(base);
    }
    if suffix == path {
        return Cow::Borrowed("");
    }
    match base.strip_suffix(suffix) {
        // Stripping the whole base would leave nothing, so node keeps it whole.
        Some(stripped) if !stripped.is_empty() => Cow::Borrowed(stripped),
        _ => Cow::Borrowed(base),
    }
}

/// The half-open byte range of the final component: trailing separators skipped, then back to the
/// separator before it.
fn base_span(path: &str) -> (usize, usize) {
    let bytes = path.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes.get(end - 1) == Some(&b'/') {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && bytes.get(start - 1) != Some(&b'/') {
        start -= 1;
    }
    (start, end)
}

/// `path.extname`: the last dot in the final component and everything after it.
///
/// Transcribed from node, because the answer is not "the text after the last dot": a leading dot
/// is a hidden file rather than an extension (`".hidden"` has none), and a component of nothing
/// but dots has one only once it is longer than `".."` -- `extname("..")` is `""` while
/// `extname("...")` is `"."`.
#[must_use]
pub fn extname(path: &str) -> Cow<'_, str> {
    let bytes = path.as_bytes();
    let mut start_dot: Option<usize> = None;
    let mut start_part = 0usize;
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    // 0: nothing but the final dot seen yet; 1: another dot seen; -1: a real character seen.
    let mut pre_dot_state = 0i32;
    for index in (0..bytes.len()).rev() {
        let byte = bytes.get(index).copied().unwrap_or(b'\0');
        if byte == b'/' {
            if !matched_slash {
                start_part = index + 1;
                break;
            }
            continue;
        }
        if end.is_none() {
            matched_slash = false;
            end = Some(index + 1);
        }
        if byte == b'.' {
            match start_dot {
                None => start_dot = Some(index),
                Some(_) if pre_dot_state != 1 => pre_dot_state = 1,
                Some(_) => {}
            }
        } else if start_dot.is_some() {
            pre_dot_state = -1;
        }
    }
    let (Some(start_dot), Some(end)) = (start_dot, end) else {
        return Cow::Borrowed("");
    };
    // A dot that opens the component is a hidden file, and `..` is the parent directory.
    if pre_dot_state == 0
        || (pre_dot_state == 1 && start_dot == end - 1 && start_dot == start_part + 1)
    {
        return Cow::Borrowed("");
    }
    Cow::Borrowed(path.get(start_dot..end).unwrap_or_default())
}

/// `path.normalize`: the same path with `.`, `..` and repeated separators resolved.
///
/// `..` is folded lexically, which is what node does and what the absence of a filesystem here
/// requires: there is nothing to ask whether a component is a symlink.
#[must_use]
pub fn normalize_posix_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|part| *part != "..") => {
                parts.pop();
            }
            ".." if !absolute => parts.push(".."),
            ".." => {}
            part => parts.push(part),
        }
    }
    let mut normalized = parts.join("/");
    if absolute {
        normalized.insert(0, '/');
    }
    if normalized.is_empty() {
        normalized.push('.');
    }
    if trailing && normalized != "/" && normalized != "." {
        normalized.push('/');
    }
    normalized
}

/// `path.relative(from, to)`: the path that reaches `to` when walked from `from`.
///
/// `..` is folded first, lexically, for the same reason `normalize` folds it: there is no
/// filesystem here to ask whether a component is a symlink.
#[must_use]
pub fn posix_path_relative(from: &str, to: &str) -> String {
    let named = |path: &str| -> Vec<String> {
        normalize_posix_path(path)
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_owned)
            .collect()
    };
    let (from, to) = (named(from), named(to));
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts: Vec<&str> = vec![".."; from.len().saturating_sub(shared)];
    parts.extend(
        to.get(shared..)
            .unwrap_or_default()
            .iter()
            .map(String::as_str),
    );
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every expectation is node's own answer, captured by running node over the matrix. A change
    /// that "fixes" one of these has changed what `node:path` means, not what this crate does.
    const DECOMPOSITIONS: &[(&str, &str, &str, &str)] = &[
        // path, dirname, basename, extname
        ("", ".", "", ""),
        ("/", "/", "", ""),
        ("a", ".", "a", ""),
        ("a/", ".", "a", ""),
        ("/a", "/", "a", ""),
        ("a/b", "a", "b", ""),
        ("a//b", "a/", "b", ""),
        ("/a/b/", "/a", "b", ""),
        ("..", ".", "..", ""),
        ("../..", "..", "..", ""),
        (".", ".", ".", ""),
        ("./a", ".", "a", ""),
        ("/a/./b", "/a/.", "b", ""),
        ("/a/../b", "/a/..", "b", ""),
        ("a.js", ".", "a.js", ".js"),
        ("/foo/bar.js", "/foo", "bar.js", ".js"),
        ("/foo/.js", "/foo", ".js", ""),
        (".hidden", ".", ".hidden", ""),
        ("/foo/bar.", "/foo", "bar.", "."),
        ("/a/b.c/d", "/a/b.c", "d", ""),
        ("//a//b//", "//a/", "b", ""),
        ("...", ".", "...", "."),
        ("a..b", ".", "a..b", ".b"),
        (".a.b", ".", ".a.b", ".b"),
        ("....", ".", "....", "."),
        ("a.", ".", "a.", "."),
        ("/..", "/", "..", ""),
        ("/.", "/", ".", ""),
        ("x/...", "x", "...", "."),
        ("..a", ".", "..a", ".a"),
    ];

    #[test]
    fn decompositions_answer_what_node_answers() {
        for (path, dir, base, ext) in DECOMPOSITIONS {
            assert_eq!(dirname(path), *dir, "dirname({path:?})");
            assert_eq!(basename(path, None), *base, "basename({path:?})");
            assert_eq!(extname(path), *ext, "extname({path:?})");
        }
    }

    #[test]
    fn a_suffix_is_removed_only_when_something_would_be_left() {
        // path, suffix, node's answer.
        let cases = [
            ("/foo/bar.js", "bar.js", "bar.js"),
            ("bar.js", "bar.js", ""),
            ("/foo/.js", ".js", ".js"),
            ("/foo/bar.js", ".js", "bar"),
            ("/a/b/", "b", "b"),
            ("/foo/bar.js", "z.js", "bar.js"),
            ("", "x", ""),
            ("/foo/bar.js", "", "bar.js"),
            ("a.js", ".js", "a"),
        ];
        for (path, suffix, want) in cases {
            assert_eq!(
                basename(path, Some(suffix)),
                want,
                "basename({path:?}, {suffix:?})"
            );
        }
    }

    #[test]
    fn normalize_answers_what_node_answers() {
        let cases = [
            ("", "."),
            ("/", "/"),
            ("a/", "a/"),
            ("a//b", "a/b"),
            ("..", ".."),
            ("../..", "../.."),
            ("./a", "a"),
            ("/a/./b", "/a/b"),
            ("/a/../b", "/b"),
            ("//a//b//", "/a/b/"),
        ];
        for (path, want) in cases {
            assert_eq!(normalize_posix_path(path), want, "normalize({path:?})");
        }
    }

    #[test]
    fn relative_answers_what_node_answers() {
        let cases = [
            ("/a/b", "/a/b/c", "c"),
            ("/a/b/c", "/a/b", ".."),
            ("/a", "/b", "../b"),
            ("a", "b", "../b"),
            ("/a/b", "/a/b", ""),
            ("/", "/a", "a"),
            ("/a/b/c", "/a/d", "../../d"),
            ("a/b", "a/b/c", "c"),
        ];
        for (from, to, want) in cases {
            assert_eq!(
                posix_path_relative(from, to),
                want,
                "relative({from:?}, {to:?})"
            );
        }
    }
}

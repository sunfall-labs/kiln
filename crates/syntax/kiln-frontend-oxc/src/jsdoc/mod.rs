//! JSDoc comments: the types of a JavaScript file. Parsed as TypeScript's `jsdoc.go` does, from
//! the comments the parser recorded; a checker reparses the tags of the comments attached to a
//! node into annotations, as `reparser.go` does.

mod scan;
mod tags;
mod types;

use kiln_hir::jsdoc::{
    CommentRange, JsDocComment, JsDocDiagnostic, JsDocDiagnosticKind, JsDocSyntax,
};

/// `isJSDocLikeText`
fn is_jsdoc_like(comment: &str) -> bool {
    let bytes = comment.as_bytes();
    bytes.len() >= 4 && bytes[1] == b'*' && bytes[2] == b'*' && bytes[3] != b'/'
}

/// The comments of `source`, as byte ranges in source order, and the tags of those that have any.
pub(crate) fn parse_comments(
    source: &str,
    ranges: impl IntoIterator<Item = (u32, u32)>,
) -> JsDocSyntax {
    let mut syntax = JsDocSyntax::default();
    for (start, end) in ranges {
        let (start, end) = (start as usize, (end as usize).min(source.len()));
        let Some(comment) = source.get(start..end) else {
            continue;
        };
        let jsdoc_like = is_jsdoc_like(comment);
        syntax.comments.push(CommentRange {
            start: start as u32,
            end: end as u32,
            jsdoc_like,
            single_line: comment.starts_with("//"),
            see_or_link: jsdoc_like && (comment.contains("@see") || comment.contains("{@link")),
        });
        if !jsdoc_like
            || !comment.ends_with("*/")
            || !comment.contains('@')
            || syntax
                .docs
                .last()
                .is_some_and(|last| last.start as usize >= start)
        {
            continue;
        }
        let read = tags::Reader::read(source, start, end);
        syntax.asterisks.extend(read.asterisks);
        syntax.docs.push(JsDocComment {
            start: start as u32,
            end: end as u32,
            tags: read.tags,
            diagnostics: read.diagnostics,
        });
    }
    syntax.asterisks.sort_unstable();
    syntax.asterisks.dedup();
    syntax
}

#[allow(dead_code)]
fn _unused(_: JsDocDiagnostic, _: JsDocDiagnosticKind) {}

#[cfg(test)]
mod tests;

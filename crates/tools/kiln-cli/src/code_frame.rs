//! A code frame for an uncaught error: the source line of the innermost stack frame, with a
//! caret under the column, printed above the engine's report.
//!
//! Both engines print the same report (`kiln: uncaught <stack>`), and the stack's frames are
//! `file:line:column` relative to the project root. The CLI is what has the sources, so it reads
//! the first frame back out of that text and the file off disk; the report itself is untouched,
//! which keeps what the engines print byte-identical. A binary from `kiln build` has no sources
//! at hand and prints the report alone.
//!
//! Only the interpreter's `kiln run` goes through here. `kiln run --native` hands the program the
//! terminal's own streams, so its report never passes through the CLI and has no frame;
//! `KILN_CODE_FRAME=0` turns the interpreter's off too, which is what `scripts/native.sh` sets so
//! that it compares the engines' reports and nothing the CLI added around one of them.

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet};
use std::path::{Path, PathBuf};

const HEADER: &str = "kiln: uncaught ";

/// `report` with a code frame inserted before the first uncaught-error header in it, or
/// unchanged when there is no header, no frame with a position, or no readable source for it.
/// `entry` is the program's entry file, from which the project root is found.
pub fn decorate(report: &str, entry: &Path) -> String {
    if std::env::var_os("KILN_CODE_FRAME").is_some_and(|value| value == "0") {
        return report.to_owned();
    }
    let Some(start) = report
        .match_indices(HEADER)
        .map(|(index, _)| index)
        .find(|&index| index == 0 || report.as_bytes()[index - 1] == b'\n')
    else {
        return report.to_owned();
    };
    match frame_for(&report[start..], entry) {
        Some(frame) => format!("{}{frame}\n{}", &report[..start], &report[start..]),
        None => report.to_owned(),
    }
}

/// Render the frame for the innermost positioned `at` line of one report.
fn frame_for(report: &str, entry: &Path) -> Option<String> {
    let mut lines = report.lines();
    let header = lines.next()?.strip_prefix(HEADER)?;
    let (file, line, column) = lines.find_map(frame_location)?;
    let source_path = resolve(&file, entry)?;
    let source = std::fs::read_to_string(&source_path).ok()?;
    render(&source, &file, line, column, header)
}

/// `    at f (dir/file.js:3:9)` or `    at dir/file.js:3:9` -> (`dir/file.js`, 3, 9).
fn frame_location(line: &str) -> Option<(String, usize, usize)> {
    let rest = line.strip_prefix("    at ")?;
    let location = match rest.strip_suffix(')') {
        Some(inner) => inner
            .rsplit_once(" (")
            .map_or(inner, |(_, location)| location),
        None => rest,
    };
    let (rest, column) = location.rsplit_once(':')?;
    let (file, line) = rest.rsplit_once(':')?;
    Some((file.to_owned(), line.parse().ok()?, column.parse().ok()?))
}

/// A frame's file is relative to a project root we were not told: the entry's directory or one
/// of its ancestors (a `package.json` sets it), or the working directory.
fn resolve(file: &str, entry: &Path) -> Option<PathBuf> {
    let candidate = Path::new(file);
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    let cwd = std::env::current_dir().ok();
    entry
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .map(Path::to_path_buf)
        .chain(cwd)
        .map(|root| root.join(candidate))
        .find(|path| path.is_file())
}

fn render(source: &str, file: &str, line: usize, column: usize, header: &str) -> Option<String> {
    let text = source.lines().nth(line.checked_sub(1)?)?;
    let line_start: usize = source
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum();
    let column_offset: usize = text
        .chars()
        .take(column.saturating_sub(1))
        .map(char::len_utf8)
        .sum();
    let start = line_start + column_offset;
    // Underline the word at the column (`new`, a callee's name), or one character.
    let word: usize = source[start..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .map(char::len_utf8)
        .sum();
    let end = start + word.max(source[start..].chars().next().map_or(0, char::len_utf8));
    if end > source.len() || start == end {
        return None;
    }
    let label = header.lines().next().unwrap_or(header);
    let report = [Level::ERROR.primary_title("uncaught exception").element(
        Snippet::source(source)
            .line_start(1)
            .path(file)
            .annotation(AnnotationKind::Primary.span(start..end).label(label)),
    )];
    // Coloured on a terminal by the same rule as `kiln check` (NO_COLOR, CLICOLOR_FORCE, TERM).
    let styled = crate::render::use_color(
        crate::args::ColorChoice::Auto,
        std::io::IsTerminal::is_terminal(&std::io::stderr()),
    );
    let renderer = if styled {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    Some(renderer.render(&report).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_and_anonymous_frames() {
        assert_eq!(
            frame_location("    at f (a/b.js:3:9)"),
            Some(("a/b.js".into(), 3, 9))
        );
        assert_eq!(
            frame_location("    at a/b.js:4:1"),
            Some(("a/b.js".into(), 4, 1))
        );
        assert_eq!(frame_location("    at <anonymous>"), None);
    }

    #[test]
    fn frames_the_throwing_line() {
        let dir = std::env::temp_dir().join(format!("kiln-code-frame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("t.js");
        std::fs::write(&entry, "function f(){ throw new Error(\"boom\") }\nf();\n").unwrap();
        let report = "kiln: uncaught Error: boom\n    at f (t.js:1:21)\n    at t.js:2:1\n";
        let out = decorate(report, &entry);
        assert!(out.contains("1 | function f(){ throw new Error"), "{out}");
        assert!(out.contains("^^^ Error: boom"), "{out}");
        assert!(out.ends_with(report), "{out}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

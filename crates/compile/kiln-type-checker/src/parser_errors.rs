//! The TypeScript diagnostics for the errors the oxc parser reports.
//!
//! Oxc words many of its errors as TypeScript does, and reports them at the place TypeScript
//! does: `A rest parameter cannot be optional` is TypeScript's 1047. Such an error is TypeScript's
//! own and the file is checked past it, so it is reported as TypeScript reports it. An oxc error
//! with no TypeScript counterpart that can be told from the message alone stays what it is, an
//! error of the frontend that makes the result incomplete.

use crate::engine::hir::DiagnosticKind;
use crate::engine::messages::message as template_of;
use kiln_hir::Diagnostic;

/// A diagnostic TypeScript reports for a parser error.
pub(crate) struct TypeScriptError {
    pub kind: DiagnosticKind,
    pub code: u32,
    pub args: Vec<String>,
    pub start: u32,
    /// 0: the end of the token at `start`. `u32::MAX`: `start`.
    pub end: u32,
}

/// The errors of `checkGrammar*` that Oxc raises from its parser with TypeScript's own words: the
/// message of each is the oxc message, with `{0}` for what varies. Not 1039: the checker reaches
/// every initializer in an ambient context itself, and reports it only when no modifier of the
/// declaration was in error first (`checkPropertyDeclaration`).
const GRAMMAR: &[u32] = &[
    1013, 1014, 1019, 1021, 1029, 1030, 1038, 1047, 1048, 1070, 1071, 1098, 1099, 1172, 1174, 1200,
    1221, 1265, 1275, 1354,
];

/// Those of the parser and the scanner.
const PARSE: &[u32] = &[1161, 1185, 1260, 1477];

pub(crate) fn typescript_error(diagnostic: &Diagnostic, source: &str) -> Option<TypeScriptError> {
    let span = diagnostic.span?;
    let (start, end) = (span.start, span.end);
    let written = source.get(start as usize..end as usize)?;
    let message = diagnostic.message.as_str();
    let error = |kind, code, args: Vec<String>| TypeScriptError {
        kind,
        code,
        args,
        start,
        end: if end == start { u32::MAX } else { end },
    };
    // `checkGrammarObjectLiteralExpression`
    if message == "Expected `,` or `}` but found `?`" {
        return Some(error(DiagnosticKind::Grammar, 1162, Vec::new()));
    }
    // `parseExpected`: `'{0}' expected.`, for the first token Oxc lists.
    if let Some(rest) = message.strip_prefix("Expected `") {
        let token = rest.split('`').next()?;
        return Some(error(DiagnosticKind::Parse, 1005, vec![token.to_owned()]));
    }
    if message == "Expected a semicolon or an implicit semicolon after a statement, but found none"
    {
        return Some(error(DiagnosticKind::Parse, 1005, vec![";".to_owned()]));
    }
    if message == "Expected 'with' in import type options" {
        return Some(error(DiagnosticKind::Parse, 1005, vec!["with".to_owned()]));
    }
    // `parseSimpleUnaryExpression`
    if message.starts_with("A unary expression with the '")
        && message.ends_with(
            "operator cannot be used as the left operand of an exponentiation expression",
        )
    {
        let operator = message["A unary expression with the '".len()..]
            .split('\'')
            .next()?;
        return Some(error(
            DiagnosticKind::Parse,
            17006,
            vec![operator.to_owned()],
        ));
    }
    // `scan`: a character that starts no token.
    if message.starts_with("Invalid Character `") {
        let units = written.chars().next()?.len_utf16() as u32;
        return Some(TypeScriptError {
            kind: DiagnosticKind::Parse,
            code: 1127,
            args: Vec::new(),
            start,
            end: start + units,
        });
    }
    // `scanIdentifierParts`: a `\` that starts no escape is a character that starts no token.
    if message == "Invalid Unicode escape sequence"
        && start > 0
        && source.as_bytes().get(start as usize - 1) == Some(&b'\\')
    {
        return Some(TypeScriptError {
            kind: DiagnosticKind::Parse,
            code: 1127,
            args: Vec::new(),
            start: start - 1,
            end: start,
        });
    }
    // `scanTemplateAndSetTokenValue`, `scanString`: the error is where the scan stopped, which is
    // the end of the file for a template and the end of the line for a string.
    if message == "Unterminated string" {
        let rest = source.get(start as usize..)?;
        let (code, stop) = match rest.chars().next()? {
            '`' => (1160, source.len()),
            _ => (
                1002,
                start as usize + rest.find(['\n', '\r']).unwrap_or(rest.len()),
            ),
        };
        return Some(TypeScriptError {
            kind: DiagnosticKind::Parse,
            code,
            args: Vec::new(),
            start: stop as u32,
            end: u32::MAX,
        });
    }
    // `parseTypeArgumentsOfExpression`: the type arguments, which end the expression Oxc reports.
    if message == "An instantiation expression cannot be followed by a property access."
        && written.ends_with('>')
    {
        let mut depth = 0usize;
        let open = written.char_indices().rev().find_map(|(at, c)| {
            match c {
                '>' => depth += 1,
                '<' => depth -= 1,
                _ => {}
            }
            (c == '<' && depth == 0).then_some(at)
        })?;
        return Some(TypeScriptError {
            kind: DiagnosticKind::Parse,
            code: 1477,
            args: Vec::new(),
            start: start + open as u32,
            end,
        });
    }
    // `checkGrammarForInOrForOfStatement`
    let for_in_or_of = |what: &str| {
        message
            .strip_prefix("Only a single declaration is allowed in a `for...")
            .filter(|rest| rest.starts_with(what))
    };
    if let Some(code) = for_in_or_of("in")
        .map(|_| 1091)
        .or_else(|| for_in_or_of("of").map(|_| 1188))
    {
        // `grammarErrorOnFirstToken(variableList.declarations[1], ..)`
        let comma = top_level_comma(written)?;
        let rest = &written[comma + 1..];
        let first = comma + 1 + (rest.len() - rest.trim_start().len());
        let token = written[first..]
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .filter(|&at| at > 0)
            .unwrap_or_else(|| written[first..].chars().next().map_or(0, char::len_utf8));
        return Some(TypeScriptError {
            kind: DiagnosticKind::Grammar,
            code,
            args: Vec::new(),
            start: start + first as u32,
            end: start + (first + token) as u32,
        });
    }
    if message == "for-in loop variable declaration may not have an initializer" {
        return Some(error(DiagnosticKind::Grammar, 1189, Vec::new()));
    }
    if message == "for-of loop variable declaration may not have an initializer" {
        return Some(error(DiagnosticKind::Grammar, 1190, Vec::new()));
    }
    // `checkGrammarNumericLiteral`: `0o` and the digits after the leading zero.
    if message == "'0'-prefixed octal literals and octal escape sequences are deprecated"
        && written.len() > 1
        && written.bytes().all(|b| (b'0'..=b'7').contains(&b))
    {
        // The operand of a prefix `-` is reported with it.
        let before = &source[..start as usize];
        let negated = before.ends_with('-')
            && !before[..before.len() - 1]
                .trim_end()
                .ends_with(|c: char| c.is_alphanumeric() || "_$)]}\"'`".contains(c));
        let (start, sign) = if negated {
            (start - 1, "-")
        } else {
            (start, "")
        };
        return Some(TypeScriptError {
            kind: DiagnosticKind::Grammar,
            code: 1121,
            args: vec![format!("{sign}0o{}", &written[1..])],
            start,
            end,
        });
    }
    // `checkGrammarNoSubstitutionTemplateLiteral`... an octal escape in a template.
    if message == "Bad escape sequence in untagged template literal"
        && let Some(digits) = written.strip_prefix('\\')
        && digits.starts_with(|c: char| ('0'..='7').contains(&c))
    {
        let octal: String = digits
            .chars()
            .take_while(|c| ('0'..='7').contains(c))
            .collect();
        let value = u32::from_str_radix(&octal, 8).ok()?;
        return Some(error(
            DiagnosticKind::Grammar,
            1487,
            vec![format!("\\x{value:02x}")],
        ));
    }
    let text = message.strip_suffix('.').unwrap_or(message);
    for (kind, codes) in [
        (DiagnosticKind::Grammar, GRAMMAR),
        (DiagnosticKind::Parse, PARSE),
    ] {
        for &code in codes {
            let Some((_, template)) = template_of(code) else {
                continue;
            };
            if let Some(args) =
                matches_template(template.strip_suffix('.').unwrap_or(template), text)
            {
                return Some(error(kind, code, args));
            }
        }
    }
    None
}

/// The arguments of `template`, a message with `{0}` placeholders, if `text` is that message.
fn matches_template(template: &str, text: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let (mut template, mut text) = (template, text);
    while let Some(open) = template.find('{') {
        let literal = &template[..open];
        text = text.strip_prefix(literal)?;
        let close = template[open..].find('}')? + open;
        template = &template[close + 1..];
        // The argument runs to what follows the placeholder.
        let following = template.split('{').next().unwrap_or_default();
        let end = if following.is_empty() {
            text.len()
        } else {
            text.find(following)?
        };
        args.push(text[..end].to_owned());
        text = &text[end..];
    }
    (text == template).then_some(args)
}

/// Where the first comma is that is not inside brackets, braces, parentheses or a string.
fn top_level_comma(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth = depth.saturating_sub(1),
            (None, ',') if depth == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

//! Reads the tags of a JSDoc comment: TypeScript's `jsdoc.go`. The tags are read with the scanner
//! of the comment (`ScanJSDocToken`), and the ordinary syntax in them (types, the clause of an
//! `@import`) with the scanner of the types, which takes over at a token and gives it back.

use super::scan::Tok;
use super::types::{Mark as TypeMark, TypeParser};
use kiln_hir::jsdoc::*;
use kiln_hir::{TypeExpr, TypeExprKind, TypeKeyword};
use oxc_syntax::identifier::{is_identifier_part, is_identifier_start};

// ───────────────────────────── tokens ─────────────────────────────

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Token {
    EndOfFile,
    Whitespace,
    NewLine,
    /// `JSDocCommentTextToken`
    CommentText,
    At,
    Asterisk,
    OpenBrace,
    CloseBrace,
    OpenBracket,
    CloseBracket,
    LessThan,
    Equals,
    Comma,
    Dot,
    DotDotDot,
    Backtick,
    Hash,
    /// An identifier or a keyword.
    Word,
    /// A character `ScanJSDocToken` does not recognize.
    Unknown,
    Other,
}

fn token_of(tok: Tok) -> Token {
    match tok {
        Tok::Eof => Token::EndOfFile,
        Tok::At => Token::At,
        Tok::Star => Token::Asterisk,
        Tok::OpenBrace => Token::OpenBrace,
        Tok::CloseBrace => Token::CloseBrace,
        Tok::OpenBracket => Token::OpenBracket,
        Tok::CloseBracket => Token::CloseBracket,
        Tok::Lt => Token::LessThan,
        Tok::Eq => Token::Equals,
        Tok::Comma => Token::Comma,
        Tok::Dot => Token::Dot,
        Tok::DotDotDot => Token::DotDotDot,
        Tok::Ident | Tok::PrivateIdent => Token::Word,
        _ => Token::Other,
    }
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_white_space_single_line(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\u{0B}' | '\u{0C}' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// The character of the unicode escape `\uXXXX` or `\u{X..}` at `at`, and its length.
fn unicode_escape_at(text: &str, at: usize) -> Option<(char, usize)> {
    let rest = text.get(at..)?.strip_prefix("\\u")?;
    if let Some(braced) = rest.strip_prefix('{') {
        let close = braced.find('}')?;
        let code = u32::from_str_radix(&braced[..close], 16).ok()?;
        return Some((char::from_u32(code)?, 3 + close + 1));
    }
    let code = u32::from_str_radix(rest.get(..4)?, 16).ok()?;
    Some((char::from_u32(code)?, 6))
}

/// The end of the identifier parts (with escapes) that continue from `at`.
fn scan_identifier_parts(text: &str, mut at: usize) -> usize {
    while let Some(c) = text.get(at..).and_then(|rest| rest.chars().next()) {
        if c == '\\' {
            match unicode_escape_at(text, at) {
                Some((decoded, len)) if is_identifier_part(decoded) => at += len,
                _ => break,
            }
        } else if is_identifier_part(c) {
            at += c.len_utf8();
        } else {
            break;
        }
    }
    at
}

/// The identifier `text` with its unicode escapes decoded.
fn unescaped_name(text: &str) -> String {
    let mut name = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(c) = text.get(at..).and_then(|rest| rest.chars().next()) {
        if c == '\\'
            && let Some((decoded, len)) = unicode_escape_at(text, at)
        {
            name.push(decoded);
            at += len;
        } else {
            name.push(c);
            at += c.len_utf8();
        }
    }
    name
}

/// `jsdocState`
#[derive(Copy, Clone, PartialEq, Eq)]
enum State {
    BeginningOfLine,
    SawAsterisk,
    SavingComments,
    SavingBackticks,
}

impl State {
    fn saving(in_fenced_code_block: bool) -> State {
        if in_fenced_code_block {
            State::SavingBackticks
        } else {
            State::SavingComments
        }
    }

    fn is_saving(self) -> bool {
        matches!(self, State::SavingComments | State::SavingBackticks)
    }
}

/// `propertyLikeParse`
const PROPERTY: u8 = 1;
const PARAMETER: u8 = 2;
const CALLBACK_PARAMETER: u8 = 4;

struct Mark {
    token: Token,
    full_start: usize,
    start: usize,
    end: usize,
    has_newline_before: bool,
    is_in_lexer: bool,
    types: TypeMark,
}

pub(super) struct Reader<'a> {
    /// The source, up to the `*/` of the comment.
    text: &'a str,
    types: TypeParser<'a>,
    token: Token,
    /// `TokenFullStart`, unless the token belongs to the scanner of the types.
    full_start: usize,
    /// `TokenStart`
    start: usize,
    /// The scanner position: `TokenEnd`.
    end: usize,
    /// `HasPrecedingLineBreak`
    has_newline_before: bool,
    /// The current token is the current token of the scanner of the types.
    is_in_lexer: bool,
}

/// What reading a comment gives.
pub(super) struct Read {
    pub tags: Vec<JsDocTag>,
    pub diagnostics: Vec<JsDocDiagnostic>,
    pub asterisks: Vec<u32>,
}

impl<'a> Reader<'a> {
    /// `parseJSDocComment` for the comment from `start` to `end` of `source`.
    pub fn read(source: &'a str, start: usize, end: usize, context: (bool, bool)) -> Read {
        let text = &source[..end - 2];
        let mut types = TypeParser::new(text, start + 3, false);
        types.await_context = context.0;
        types.yield_context = context.1;
        let mut reader = Reader {
            text,
            types,
            token: Token::Unknown,
            full_start: start + 3,
            start: start + 3,
            end: start + 3,
            has_newline_before: false,
            is_in_lexer: false,
        };
        let tags = reader.comment(start);
        let mut diagnostics = std::mem::take(&mut reader.types.diagnostics);
        // `A.<T>` is equivalent to `A<T>` in a JSDoc comment (`checkTypeReferenceNode`).
        diagnostics.retain(|diagnostic| diagnostic.code != 8020);
        Read {
            tags,
            diagnostics,
            asterisks: std::mem::take(&mut reader.types.s.asterisks),
        }
    }

    // ───────────────────────────── errors ─────────────────────────────

    /// `parseErrorAt`
    fn error(&mut self, start: usize, len: usize, code: u32) {
        self.types.error_at(start, len, code, &[]);
    }

    /// `parseErrorAtCurrentToken`
    fn error_at_token(&mut self, code: u32) {
        self.error(self.start, self.end - self.start, code);
    }

    /// `parseErrorAtCurrentToken`, of `'{0}' expected.`
    fn expected_at_token(&mut self, token: &str) {
        self.types
            .error_at(self.start, self.end - self.start, 1005, &[token]);
    }

    /// `'{0}' tag already specified.`, of the tag named `name`, up to the current token.
    fn tag_already_specified(&mut self, name: &JsDocName) {
        let len = self.start.saturating_sub(name.start as usize);
        self.types
            .error_at(name.start as usize, len, 1223, &[&name.text]);
    }

    // ───────────────────────────── scanning ─────────────────────────────

    /// `nodePos`
    fn full_start(&self) -> usize {
        if self.is_in_lexer {
            self.types.s.full_start
        } else {
            self.full_start
        }
    }

    /// Where the current token starts, which at the end of the comment `TokenStart` is not.
    fn token_pos(&self) -> usize {
        self.start.max(self.full_start())
    }

    fn token_len(&self) -> usize {
        self.end - self.start
    }

    fn token_text(&self) -> &'a str {
        self.text.get(self.start..self.end).unwrap_or("")
    }

    /// `TokenValue`, of a word.
    fn token_value(&self) -> String {
        let text = self.token_text();
        if text.contains('\\') {
            return unescaped_name(text);
        }
        text.to_owned()
    }

    fn mark(&self) -> Mark {
        Mark {
            token: self.token,
            full_start: self.full_start,
            start: self.start,
            end: self.end,
            has_newline_before: self.has_newline_before,
            is_in_lexer: self.is_in_lexer,
            types: self.types.mark(),
        }
    }

    fn rewind(&mut self, mark: &Mark) {
        self.token = mark.token;
        self.full_start = mark.full_start;
        self.start = mark.start;
        self.end = mark.end;
        self.has_newline_before = mark.has_newline_before;
        self.is_in_lexer = mark.is_in_lexer;
        self.types.rewind(&mark.types);
    }

    fn look_ahead<R>(&mut self, scan: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.mark();
        let result = scan(self);
        self.rewind(&mark);
        result
    }

    /// `ResetPos`: the token stays what it was.
    fn reset_pos(&mut self, pos: usize) {
        self.full_start = pos;
        self.start = pos;
        self.end = pos;
        self.is_in_lexer = false;
    }

    /// `nextTokenJSDoc`, `ScanJSDocToken`
    fn next_jsdoc(&mut self) -> Token {
        let text = self.text;
        let pos = self.end;
        self.is_in_lexer = false;
        self.full_start = pos;
        self.has_newline_before = false;
        // At the end `tokenStart` stays that of the token before.
        let Some(c) = text.get(pos..).and_then(|rest| rest.chars().next()) else {
            self.token = Token::EndOfFile;
            return self.token;
        };
        self.start = pos;
        let mut end = pos + c.len_utf8();
        self.token = match c {
            ' ' | '\t' | '\u{0B}' | '\u{0C}' => {
                while let Some(c) = text.get(end..).and_then(|rest| rest.chars().next()) {
                    if !is_white_space_single_line(c) {
                        break;
                    }
                    end += c.len_utf8();
                }
                Token::Whitespace
            }
            '\r' | '\n' => {
                if c == '\r' && text[end..].starts_with('\n') {
                    end += 1;
                }
                self.has_newline_before = true;
                Token::NewLine
            }
            '@' => Token::At,
            '*' => Token::Asterisk,
            '{' => Token::OpenBrace,
            '}' => Token::CloseBrace,
            '[' => Token::OpenBracket,
            ']' => Token::CloseBracket,
            '<' => Token::LessThan,
            '=' => Token::Equals,
            ',' => Token::Comma,
            '.' => Token::Dot,
            '`' => Token::Backtick,
            '#' => Token::Hash,
            '(' | ')' | '>' => Token::Other,
            '\\' => match unicode_escape_at(text, pos) {
                Some((decoded, len)) if is_identifier_start(decoded) => {
                    end = scan_identifier_parts(text, pos + len);
                    Token::Word
                }
                _ => Token::Unknown,
            },
            c if is_identifier_start(c) => {
                while let Some(c) = text.get(end..).and_then(|rest| rest.chars().next()) {
                    if !(is_identifier_part(c) || c == '-') {
                        break;
                    }
                    end += c.len_utf8();
                }
                if text[end..].starts_with('\\') {
                    end = scan_identifier_parts(text, end);
                }
                Token::Word
            }
            _ => Token::Unknown,
        };
        self.end = end;
        self.token
    }

    /// `nextJSDocCommentTextToken`, `ScanJSDocCommentTextToken`
    fn next_comment_text(&mut self, in_backticks: bool) -> Token {
        let text = self.text;
        let pos = self.end;
        let mut end = pos;
        while let Some(c) = text.get(end..).and_then(|rest| rest.chars().next()) {
            if is_line_break(c) || c == '`' {
                break;
            }
            if !in_backticks {
                if c == '{' {
                    break;
                }
                // Elsewhere `@` only starts a tag after whitespace and before an identifier.
                if c == '@'
                    && text[..end]
                        .chars()
                        .next_back()
                        .is_some_and(is_white_space_single_line)
                    && text[end + 1..]
                        .chars()
                        .next()
                        .is_some_and(is_identifier_start)
                {
                    break;
                }
            }
            end += c.len_utf8();
        }
        if end == pos {
            return self.next_jsdoc();
        }
        self.is_in_lexer = false;
        self.full_start = pos;
        self.start = pos;
        self.end = end;
        self.has_newline_before = false;
        self.token = Token::CommentText;
        self.token
    }

    /// `CanFollowJSDocAt`
    fn can_follow_at(&self) -> bool {
        match self
            .text
            .get(self.end..)
            .and_then(|rest| rest.chars().next())
        {
            None => true,
            Some(c) => is_identifier_start(c) || is_white_space_single_line(c) || is_line_break(c),
        }
    }

    /// Takes the token from the scanner of the types.
    fn leave_lexer(&mut self) {
        let scanner = &self.types.s;
        self.token = token_of(scanner.tok);
        self.start = scanner.start;
        self.end = scanner.end;
        self.has_newline_before = scanner.newline_before;
        self.is_in_lexer = true;
    }

    /// Makes the scanner of the types scan the token at or after `pos`.
    fn scan_from(&mut self, pos: usize) {
        self.types.reset_to(pos);
        self.types.next_token();
        self.leave_lexer();
    }

    /// Called before the types continue from the current token: their scanner scans that token
    /// unless it already has.
    fn enter_lexer(&mut self) {
        if self.is_in_lexer {
            return;
        }
        let (token, full_start, end) = (self.token, self.full_start, self.end);
        let word = (token == Token::Word).then(|| self.token_value());
        self.scan_from(full_start);
        // The token stays what `ScanJSDocToken` returned: its words can contain `-`, and such a
        // word is no keyword.
        if token == Token::Word && end != self.end && self.types.s.tok == Tok::Ident {
            self.types.s.set_token_end(end, word.unwrap_or_default());
            self.start = full_start;
            self.end = end;
        }
    }

    /// `nextToken`
    fn next_token(&mut self) {
        if self.is_in_lexer {
            self.types.next_token();
            self.leave_lexer();
        } else {
            self.scan_from(self.end);
        }
    }

    /// `parseOptional`
    fn eat(&mut self, token: Token) -> bool {
        if self.token == token {
            self.next_token();
            return true;
        }
        false
    }

    /// `parseExpected`
    fn expect(&mut self, token: Token) -> bool {
        if self.eat(token) {
            return true;
        }
        self.expected_at_token(match token {
            Token::At => "@",
            Token::Asterisk => "*",
            Token::OpenBrace => "{",
            Token::CloseBrace => "}",
            Token::OpenBracket => "[",
            Token::CloseBracket => "]",
            Token::LessThan => "<",
            Token::Equals => "=",
            Token::Comma => ",",
            Token::Dot => ".",
            Token::DotDotDot => "...",
            Token::Backtick => "`",
            _ => "",
        });
        false
    }

    /// `parseOptionalJsdoc`
    fn eat_jsdoc(&mut self, token: Token) -> bool {
        if self.token == token {
            self.next_jsdoc();
            return true;
        }
        false
    }

    // ───────────────────────────── syntax read by the types ─────────────────────────────

    /// `SetSkipJSDocLeadingAsterisks`
    fn set_skips_leading_asterisks(&mut self, skips: bool) {
        self.types.s.skip_asterisks = skips;
    }

    /// `parseTypeOrTypePredicate`, starting at the current token.
    fn read_type(&mut self) -> Option<TypeExpr> {
        // `parseTypeReference` at a token of `ScanJSDocToken` that starts no type, or at the end:
        // the name is missing (`createIdentifierWithDiagnostic`), and the token is not consumed.
        let starts_no_type = matches!(
            self.token,
            Token::Unknown | Token::Whitespace | Token::NewLine | Token::Backtick | Token::Hash
        );
        if !self.is_in_lexer && (starts_no_type || self.token == Token::EndOfFile) {
            let at = self.full_start;
            if starts_no_type {
                self.error_at_token(1110);
            } else {
                self.error(at, 0, 1110);
            }
            return Some(TypeExpr {
                span: kiln_hir::Span::new(at as u32, at as u32),
                kind: TypeExprKind::Reference {
                    name: vec![String::new()],
                    arguments: Vec::new(),
                },
            });
        }
        self.enter_lexer();
        let ty = self.types.parse_type_or_predicate();
        self.leave_lexer();
        Some(ty)
    }

    /// `parseTypeArguments`, starting at the current token.
    fn read_type_arguments(&mut self) -> Option<Vec<TypeExpr>> {
        if self.token != Token::LessThan {
            return None;
        }
        self.enter_lexer();
        let arguments = self.types.parse_type_arguments();
        self.leave_lexer();
        arguments
    }

    /// `parseExpression`, starting at the current token. Its extent is all that is used.
    fn read_expression(&mut self) {
        self.enter_lexer();
        let mut depth = 0usize;
        loop {
            match self.types.s.tok {
                Tok::Eof => break,
                Tok::OpenParen | Tok::OpenBracket | Tok::OpenBrace => depth += 1,
                Tok::CloseParen | Tok::CloseBrace => depth = depth.saturating_sub(1),
                Tok::CloseBracket => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Tok::Comma if depth == 0 => break,
                _ => {}
            }
            self.types.next_token();
        }
        self.leave_lexer();
    }

    /// `isIdentifier`, of the word `word`.
    fn is_identifier(&self, word: &str) -> bool {
        !super::scan::Scanner::is_reserved_word(word)
            && !(word == "await" && self.types.await_context)
            && !(word == "yield" && self.types.yield_context)
    }

    /// `parseImportTag`, starting at the token after the tag name.
    fn read_import(&mut self) -> JsDocImport {
        // Either only whitespace remains in the comment (`skipWhitespaceOrAsterisk` does not
        // consume it), or `ScanJSDocToken` returned `KindUnknown`, for example for a quote or a
        // slash, or a token of its own: `#a` is no private name and a backtick opens no template.
        // `parseModuleSpecifier` reports TS1109 without consuming the token.
        let cannot_start_expression = matches!(
            self.token,
            Token::Whitespace | Token::NewLine | Token::Unknown | Token::Hash | Token::Backtick
        );
        if !self.is_in_lexer && cannot_start_expression {
            self.error_at_token(1109);
            return JsDocImport {
                end: self.start as u32,
                ..JsDocImport::default()
            };
        }
        let mut import = JsDocImport {
            clause_start: self.token_pos() as u32,
            ..JsDocImport::default()
        };
        // `isIdentifier`, `parseIdentifier`: the token after the tag name is one of
        // `ScanJSDocToken`, whose words can contain `-`.
        if self.token == Token::Word && self.is_identifier(&self.token_value()) {
            import.default = Some(self.name_at_word());
            self.next_token();
        }
        self.enter_lexer();
        self.read_import_declaration(&mut import);
        self.types.s.skip_asterisks = false;
        self.leave_lexer();
        import.end = self.start as u32;
        import
    }

    fn read_import_declaration(&mut self, import: &mut JsDocImport) {
        // `tryParseImportClause`
        if import.default.is_some() || matches!(self.types.s.tok, Tok::Star | Tok::OpenBrace) {
            import.has_clause = true;
            // `parseImportClause`
            let has_bindings = if import.default.is_none() {
                true
            } else if self.types.s.tok == Tok::Comma {
                self.types.next_token();
                true
            } else {
                false
            };
            if has_bindings {
                self.types.s.skip_asterisks = true;
                if self.types.s.tok == Tok::Star {
                    // `parseNamespaceImport`
                    import.namespace_start = self.types.s.start as u32;
                    self.types.next_token();
                    self.types.expect_contextual_keyword("as");
                    if self.types.is_identifier_token() {
                        import.namespace = Some(self.types.name_at_token());
                        self.types.next_token();
                    } else {
                        let at = self.types.s.full_start as u32;
                        import.namespace = Some(JsDocName::missing(at));
                        self.types.expect_token(Tok::Ident);
                    }
                } else {
                    // `parseNamedImports`
                    import.has_named_imports = true;
                    import.named = self.types.parse_named_imports();
                }
                self.types.s.skip_asterisks = false;
            }
            import.clause_end = self.types.s.full_start as u32;
            self.types.expect_contextual_keyword("from");
        }
        // `parseModuleSpecifier`, `tryParseImportAttributes`
        if self.types.s.tok == Tok::Str {
            let text = self.types.s.cooked_string();
            import.specifier = Some((text, self.types.s.start as u32));
            self.types.next_token();
        } else if self.types.s.tok == Tok::Eof || self.types.s.newline_before {
            self.types.error_at_current_token(1109);
        } else if import.has_clause || self.types.s.tok != Tok::Ident {
            self.types.error_at_current_token(1141);
            self.types.next_token();
        } else {
            // `await from "./b"`, where `await` is no name: an expression.
            self.types.next_token();
        }
        if self.types.is_word_token("with") || self.types.is_word_token("assert") {
            import.mode = Some(self.types.parse_import_attributes());
        }
    }

    fn is_object_or_object_array(&self, ty: &Option<JsDocTypeExpr>) -> bool {
        match ty {
            Some(expr) if !expr.is_variadic && !expr.is_optional => {
                expr.ty.as_ref().is_some_and(is_object_or_object_array)
            }
            _ => false,
        }
    }

    fn is_array_type(&self, ty: &Option<JsDocTypeExpr>) -> bool {
        match ty {
            Some(expr) if !expr.is_variadic && !expr.is_optional => expr
                .ty
                .as_ref()
                .is_some_and(|ty| matches!(ty.kind, TypeExprKind::Array(_))),
            _ => false,
        }
    }

    // ───────────────────────────── comments ─────────────────────────────

    /// `parseJSDocCommentWorker`. Only the state that determines where tags start is tracked, not
    /// the text.
    fn comment(&mut self, start: usize) -> Vec<JsDocTag> {
        let line_start = self.text[..start].rfind('\n').map_or(0, |at| at + 1);
        // For the leading `/** `.
        let mut indent = start + 4 - line_start;
        let mut tags: Vec<JsDocTag> = Vec::new();
        let mut state = State::SawAsterisk;
        let mut backticks = 0;
        let mut in_fenced_code_block = false;

        self.next_jsdoc();
        while self.eat_jsdoc(Token::Whitespace) {}
        if self.eat_jsdoc(Token::NewLine) {
            state = State::BeginningOfLine;
            indent = 0;
        }
        loop {
            // Three or more backticks in a row open or close a fenced code block.
            if self.token != Token::Backtick && backticks > 0 {
                if backticks >= 3 {
                    in_fenced_code_block = !in_fenced_code_block;
                }
                backticks = 0;
            }
            match self.token {
                Token::At if !in_fenced_code_block && self.can_follow_at() => {
                    let tag = self.tag(&tags, indent);
                    tags.push(tag);
                    state = State::BeginningOfLine;
                }
                Token::NewLine => {
                    state = State::BeginningOfLine;
                    indent = 0;
                }
                Token::Asterisk if state != State::SawAsterisk => {
                    // The first asterisk of a line.
                    state = State::SawAsterisk;
                    indent += self.token_len();
                }
                Token::Whitespace => indent += self.token_len(),
                Token::EndOfFile => break,
                Token::Backtick => {
                    backticks += 1;
                    state = if state == State::SavingBackticks {
                        State::SavingComments
                    } else {
                        State::SavingBackticks
                    };
                    indent += self.token_len();
                }
                Token::Asterisk => {
                    // After a second asterisk no tag starts on the line.
                    state = State::SavingComments;
                    indent += self.token_len();
                }
                Token::OpenBrace if !in_fenced_code_block => {
                    state = State::SavingComments;
                    if !self.link() {
                        indent += self.token_len();
                    }
                }
                Token::At | Token::OpenBrace => {
                    state = State::saving(in_fenced_code_block);
                    indent += self.token_len();
                }
                _ => {
                    if state != State::SavingBackticks {
                        state = State::saving(in_fenced_code_block);
                    }
                    indent += self.token_len();
                }
            }
            if state.is_saving() {
                self.next_comment_text(state == State::SavingBackticks);
            } else {
                self.next_jsdoc();
            }
        }
        tags
    }

    /// `isNextNonwhitespaceTokenEndOfFile`
    fn is_next_nonwhitespace_token_end_of_file(&mut self) -> bool {
        loop {
            match self.next_jsdoc() {
                Token::EndOfFile => return true,
                Token::Whitespace | Token::NewLine => {}
                _ => return false,
            }
        }
    }

    /// Whether only trailing whitespace remains, which belongs to no node.
    fn is_at_trailing_whitespace(&mut self) -> bool {
        matches!(self.token, Token::Whitespace | Token::NewLine)
            && self.look_ahead(Self::is_next_nonwhitespace_token_end_of_file)
    }

    /// `skipWhitespace`
    fn skip_whitespace(&mut self) {
        if self.is_at_trailing_whitespace() {
            return;
        }
        while matches!(self.token, Token::Whitespace | Token::NewLine) {
            self.next_jsdoc();
        }
    }

    /// `skipWhitespaceOrAsterisk`. Returns the length of the indentation after the last line break.
    fn skip_whitespace_or_asterisk(&mut self) -> usize {
        if self.is_at_trailing_whitespace() {
            return 0;
        }
        let mut preceding_line_break = self.has_newline_before;
        let mut seen_line_break = false;
        let mut indent = 0;
        while (preceding_line_break && self.token == Token::Asterisk)
            || matches!(self.token, Token::Whitespace | Token::NewLine)
        {
            indent += self.token_len();
            if self.token == Token::NewLine {
                preceding_line_break = true;
                seen_line_break = true;
                indent = 0;
            } else if self.token == Token::Asterisk {
                preceding_line_break = false;
            }
            self.next_jsdoc();
        }
        if seen_line_break { indent } else { 0 }
    }

    /// `parseTrailingTagComments` for the tag that starts at `start`. `indent_text`: the length of
    /// that text.
    fn trailing_tag_comments(
        &mut self,
        start: usize,
        end: usize,
        mut margin: usize,
        indent_text: usize,
    ) -> bool {
        if indent_text == 0 {
            margin += end.saturating_sub(start);
        }
        self.tag_comments(margin, Some(indent_text.saturating_sub(margin)))
    }

    /// The same, up to the current token.
    fn trailing_comments(&mut self, start: usize, margin: usize, indent_text: usize) {
        let end = self.full_start();
        self.trailing_tag_comments(start, end, margin, indent_text);
    }

    /// `parseTagComments`. Returns whether there is a comment. `initial_margin`: the length of
    /// that text.
    fn tag_comments(&mut self, mut indent: usize, initial_margin: Option<usize>) -> bool {
        let mut state = State::BeginningOfLine;
        let mut backticks = 0;
        let mut in_fenced_code_block = false;
        let mut margin: Option<usize> = None;
        let mut has_text = false;
        if let Some(initial_margin) = initial_margin {
            // Straight to saving comments if there is some initial indentation.
            if initial_margin != 0 {
                margin = Some(indent);
                indent += initial_margin;
            }
            state = State::SawAsterisk;
        }
        loop {
            if self.token != Token::Backtick && backticks > 0 {
                if backticks >= 3 {
                    in_fenced_code_block = !in_fenced_code_block;
                }
                backticks = 0;
            }
            // `pushComment`
            let mut is_text = true;
            match self.token {
                Token::NewLine => {
                    state = State::BeginningOfLine;
                    indent = 0;
                    is_text = false;
                }
                Token::At if !in_fenced_code_block && self.can_follow_at() => {
                    self.reset_pos(self.end - 1);
                    break;
                }
                Token::EndOfFile => break,
                Token::Whitespace => {
                    // Whitespace that crosses the margin is part of the comment.
                    if margin.is_some_and(|margin| indent + self.token_len() > margin) {
                        state = State::saving(in_fenced_code_block);
                    }
                    indent += self.token_len();
                    is_text = false;
                }
                Token::OpenBrace if !in_fenced_code_block => {
                    state = State::SavingComments;
                    if self.link() {
                        is_text = false;
                        has_text = true;
                    }
                }
                Token::At | Token::OpenBrace => state = State::saving(in_fenced_code_block),
                Token::Backtick => {
                    backticks += 1;
                    state = if state == State::SavingBackticks {
                        State::SavingComments
                    } else {
                        State::SavingBackticks
                    };
                }
                Token::Asterisk if state == State::BeginningOfLine => {
                    // A leading asterisk: the comment continues at the next token.
                    state = State::SawAsterisk;
                    indent += 1;
                    is_text = false;
                }
                _ => {
                    if state != State::SavingBackticks {
                        state = State::saving(in_fenced_code_block);
                    }
                }
            }
            if is_text {
                margin.get_or_insert(indent);
                indent += self.token_len();
                has_text |= !self.token_text().trim_ascii().is_empty();
            }
            if state.is_saving() {
                self.next_comment_text(state == State::SavingBackticks);
            } else {
                self.next_jsdoc();
            }
        }
        has_text
    }

    /// `parseJSDocLinkPrefix`: whether `{@link`, `{@linkcode` or `{@linkplain` is next.
    fn is_at_link(&mut self) -> bool {
        self.skip_whitespace_or_asterisk();
        self.token == Token::OpenBrace
            && self.next_jsdoc() == Token::At
            && self.next_jsdoc() == Token::Word
            && matches!(
                self.token_value().as_str(),
                "link" | "linkcode" | "linkplain"
            )
    }

    /// `parseJSDocLink`, up to the `}`, the line break or the end that closes it. False: there is
    /// none, and nothing is consumed.
    fn link(&mut self) -> bool {
        let mark = self.mark();
        if !self.is_at_link() {
            self.rewind(&mark);
            return false;
        }
        self.next_jsdoc();
        self.skip_whitespace();
        self.link_name();
        while !matches!(
            self.token,
            Token::CloseBrace | Token::NewLine | Token::EndOfFile
        ) {
            self.next_jsdoc();
        }
        true
    }

    /// `parseJSDocLinkName`
    fn link_name(&mut self) {
        if self.token != Token::Word {
            return;
        }
        self.link_identifier(true);
        while self.eat(Token::Dot) {
            // `createMissingIdentifier`
            if !self.is_at_private_identifier() {
                self.link_identifier(true);
            }
        }
        while self.is_at_private_identifier() {
            // `ReScanHashToken`
            self.reset_pos(self.start + 1);
            self.next_jsdoc();
            self.link_identifier(false);
        }
    }

    /// `parseIdentifierName`, or `parseIdentifier` unless `allows_reserved_words`. A missing name
    /// is reported, and the token is not consumed.
    fn link_identifier(&mut self, allows_reserved_words: bool) {
        let is_word = self.token == Token::Word && !self.is_at_private_identifier();
        let is_reserved_word =
            is_word && super::scan::Scanner::is_reserved_word(&self.token_value());
        if is_word && (allows_reserved_words || !is_reserved_word) {
            self.next_token();
        } else if self.token == Token::EndOfFile {
            let at = self.full_start();
            self.error(at, 0, 1003);
        } else if is_reserved_word {
            let text = self.token_text();
            self.types
                .error_at(self.start, self.end - self.start, 1359, &[text]);
        } else {
            self.error_at_token(1003);
        }
    }

    /// `KindPrivateIdentifier`, which only `Scan` returns.
    fn is_at_private_identifier(&self) -> bool {
        self.is_in_lexer && self.types.s.tok == Tok::PrivateIdent
    }

    // ───────────────────────────── tags ─────────────────────────────

    /// `parseTag`
    fn tag(&mut self, previous: &[JsDocTag], margin: usize) -> JsDocTag {
        let start = self.start;
        self.next_jsdoc();
        let name = self.identifier_name(Some(1003));
        let indent_text = self.skip_whitespace_or_asterisk();
        let kind = match name.text.as_str() {
            "implements" => {
                let class = self.class_name();
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Implements(class)
            }
            "augments" | "extends" => {
                let class = self.class_name();
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Augments(class, name.text.clone())
            }
            word @ ("public" | "private" | "protected" | "readonly" | "override") => {
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Modifier(match word {
                    "public" => JsDocModifier::Public,
                    "private" => JsDocModifier::Private,
                    "protected" => JsDocModifier::Protected,
                    "readonly" => JsDocModifier::Readonly,
                    _ => JsDocModifier::Override,
                })
            }
            "this" => return self.this_tag(start, name, margin, indent_text),
            "arg" | "argument" | "param" => {
                return self.parameter_or_property_tag(start, name, PARAMETER, margin);
            }
            "return" | "returns" => {
                // `parseReturnTag`
                if previous
                    .iter()
                    .any(|tag| matches!(tag.kind, JsDocTagKind::Return(_)))
                {
                    self.tag_already_specified(&name);
                }
                let ty = self.try_type_expression();
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Return(ty)
            }
            "template" => return self.template_tag(start, name, margin, indent_text),
            "type" => return self.type_tag(previous, start, name, Some(margin), indent_text),
            "typedef" => self.typedef_tag(start, &name, margin, indent_text),
            "callback" => self.callback_tag(start, margin, indent_text),
            "overload" => self.overload_tag(start, margin, indent_text),
            "satisfies" => {
                let ty = self.type_expression(false);
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Satisfies(ty)
            }
            "exception" | "throws" => {
                self.try_type_expression();
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Other
            }
            "import" => {
                let import = self.read_import();
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Import(import)
            }
            _ => {
                self.trailing_comments(start, margin, indent_text);
                JsDocTagKind::Other
            }
        };
        JsDocTag {
            kind,
            pos: start as u32,
            name_pos: name.start,
            end: self.full_start() as u32,
        }
    }

    /// `parseJSDocIdentifierName`. `code`: the error reported if the name is missing.
    fn identifier_name(&mut self, code: Option<u32>) -> JsDocName {
        if self.token != Token::Word {
            if let Some(code) = code {
                self.error_at_token(code);
            }
            return JsDocName::missing(self.full_start() as u32);
        }
        let name = self.name_at_word();
        self.next_jsdoc();
        name
    }

    /// The current token, a word, as a name.
    fn name_at_word(&self) -> JsDocName {
        JsDocName {
            start: self.start as u32,
            end: self.end as u32,
            text: self.token_value(),
        }
    }

    /// `parseJSDocEntityName`
    fn entity_name(&mut self, code: Option<u32>) -> Vec<JsDocName> {
        let mut names = vec![self.identifier_name(code)];
        // `y[]` is accepted as a name. The brackets are dropped.
        if self.eat(Token::OpenBracket) {
            self.expect(Token::CloseBracket);
        }
        while self.eat(Token::Dot) {
            names.push(self.identifier_name(Some(1003)));
            if self.eat(Token::OpenBracket) {
                self.expect(Token::CloseBracket);
            }
        }
        names
    }

    /// `parseJSDocType`
    fn jsdoc_type(&mut self) -> JsDocTypeExpr {
        self.set_skips_leading_asterisks(true);
        let pos = self.token_pos() as u32;
        let is_variadic = self.eat(Token::DotDotDot);
        let ty = self.read_type();
        self.set_skips_leading_asterisks(false);
        let is_optional = self.eat(Token::Equals);
        JsDocTypeExpr {
            ty,
            pos,
            end: self.token_pos() as u32,
            is_variadic,
            is_optional,
        }
    }

    /// `parseJSDocTypeExpression`
    fn type_expression(&mut self, may_omit_braces: bool) -> JsDocTypeExpr {
        let has_brace = if may_omit_braces {
            self.eat(Token::OpenBrace)
        } else {
            self.expect(Token::OpenBrace)
        };
        let ty = self.jsdoc_type();
        // `parseExpectedJSDoc`
        if has_brace && !self.eat_jsdoc(Token::CloseBrace) {
            self.expected_at_token("}");
        }
        ty
    }

    /// `tryParseTypeExpression`
    fn try_type_expression(&mut self) -> Option<JsDocTypeExpr> {
        self.skip_whitespace_or_asterisk();
        (self.token == Token::OpenBrace).then(|| self.type_expression(false))
    }

    /// `parseBracketNameInPropertyAndParamTag`
    fn bracket_name(&mut self, target: u8) -> (Vec<JsDocName>, bool) {
        // `[foo]`, `foo`, `[foo.bar]` or `foo.bar`
        let is_bracketed = self.eat_jsdoc(Token::OpenBracket);
        if is_bracketed {
            self.skip_whitespace();
        }
        // A name in backquotes is not legal JSDoc, but occurs in the wild.
        let is_backquoted = self.eat_jsdoc(Token::Backtick);
        let name = self.entity_name(if target == PARAMETER {
            None
        } else {
            Some(1003)
        });
        if is_backquoted {
            self.expect(Token::Backtick);
        }
        if is_bracketed {
            self.skip_whitespace();
            // `[foo = 42]`
            if self.eat(Token::Equals) {
                self.read_expression();
            }
            self.expect(Token::CloseBracket);
        }
        (name, is_bracketed)
    }

    /// `parseParameterOrPropertyTag`
    fn parameter_or_property_tag(
        &mut self,
        start: usize,
        tag_name: JsDocName,
        target: u8,
        indent: usize,
    ) -> JsDocTag {
        let mut ty = self.try_type_expression();
        let mut is_name_first = ty.is_none();
        self.skip_whitespace_or_asterisk();
        let (name, is_bracketed) = self.bracket_name(target);
        let indent_text = self.skip_whitespace_or_asterisk();
        if is_name_first && !self.look_ahead(Self::is_at_link) {
            ty = self.try_type_expression();
        }
        self.trailing_comments(start, indent, indent_text);
        let ty = match self.nested_type_literal(&ty, &name, target, indent) {
            Some(literal) => {
                is_name_first = true;
                literal
            }
            None => ty.map_or(JsDocTagType::None, JsDocTagType::Expr),
        };
        let property = JsDocProperty {
            name,
            is_bracketed,
            is_name_first,
            ty,
            comment: String::new(),
        };
        JsDocTag {
            kind: if target == PROPERTY {
                JsDocTagKind::Property(property)
            } else {
                JsDocTagKind::Param(property)
            },
            pos: start as u32,
            name_pos: tag_name.start,
            end: self.full_start() as u32,
        }
    }

    /// `parseNestedTypeLiteral`
    fn nested_type_literal(
        &mut self,
        ty: &Option<JsDocTypeExpr>,
        name: &[JsDocName],
        target: u8,
        indent: usize,
    ) -> Option<JsDocTagType> {
        if !self.is_object_or_object_array(ty) {
            return None;
        }
        let pos = self.full_start() as u32;
        let mut properties = Vec::new();
        loop {
            let mark = self.mark();
            let Some(child) = self.child_tag(target, indent, Some(name)) else {
                self.rewind(&mark);
                break;
            };
            match child.kind {
                JsDocTagKind::Param(_) | JsDocTagKind::Property(_) => properties.push(child),
                JsDocTagKind::Template(_) => {
                    self.error(child.name_pos as usize, "template".len(), 8039)
                }
                _ => {}
            }
        }
        if properties.is_empty() {
            return None;
        }
        Some(JsDocTagType::Literal {
            properties,
            is_array: self.is_array_type(ty),
            pos,
        })
    }

    /// `parseTypeTag`. Without an `indent` the trailing comment is not consumed: the tag is nested
    /// under a `@typedef`.
    fn type_tag(
        &mut self,
        previous: &[JsDocTag],
        start: usize,
        name: JsDocName,
        indent: Option<usize>,
        indent_text: usize,
    ) -> JsDocTag {
        if previous
            .iter()
            .any(|tag| matches!(tag.kind, JsDocTagKind::Type(_)))
        {
            self.tag_already_specified(&name);
        }
        let ty = self.type_expression(true);
        if let Some(indent) = indent {
            self.trailing_comments(start, indent, indent_text);
        }
        JsDocTag {
            kind: JsDocTagKind::Type(ty),
            pos: start as u32,
            name_pos: name.start,
            end: self.full_start() as u32,
        }
    }

    /// `parseThisTag`
    fn this_tag(
        &mut self,
        start: usize,
        name: JsDocName,
        margin: usize,
        indent_text: usize,
    ) -> JsDocTag {
        let ty = self.type_expression(true);
        self.skip_whitespace();
        self.trailing_comments(start, margin, indent_text);
        let end = self.full_start();
        JsDocTag {
            kind: JsDocTagKind::This(ty, self.text[start..end].to_owned()),
            pos: start as u32,
            name_pos: name.start,
            end: end as u32,
        }
    }

    /// `parseExpressionWithTypeArgumentsForAugments`
    fn class_name(&mut self) -> JsDocClassName {
        let used_brace = self.eat(Token::OpenBrace);
        // `parsePropertyAccessEntityNameExpression`
        let mut name = vec![self.identifier_name(Some(1003))];
        while self.eat(Token::Dot) {
            name.push(self.identifier_name(Some(1003)));
        }
        self.set_skips_leading_asterisks(true);
        let type_args = self.read_type_arguments();
        self.set_skips_leading_asterisks(false);
        let end = self.token_pos() as u32;
        if used_brace {
            self.skip_whitespace();
            self.expect(Token::CloseBrace);
        }
        JsDocClassName {
            name,
            type_args,
            end,
        }
    }

    /// `parseJSDocTypeNameWithNamespace`, or a missing name.
    fn declared_name(&mut self) -> JsDocDeclaredName {
        if self.token != Token::Word {
            return JsDocDeclaredName {
                namespaces: Vec::new(),
                name: self.identifier_name(Some(1003)),
            };
        }
        let mut namespaces = Vec::new();
        let mut name = self.identifier_name(None);
        while self.eat_jsdoc(Token::Dot) {
            namespaces.push(name);
            // `getInnermostNameOfJSDocNamespace`: after a last dot, the name of the namespace
            // once more.
            if self.token != Token::Word {
                name = namespaces
                    .last()
                    .cloned()
                    .unwrap_or_else(|| JsDocName::missing(0));
                break;
            }
            name = self.identifier_name(None);
        }
        JsDocDeclaredName { namespaces, name }
    }

    /// `parseTypedefTag`
    fn typedef_tag(
        &mut self,
        start: usize,
        tag_name: &JsDocName,
        indent: usize,
        indent_text: usize,
    ) -> JsDocTagKind {
        let written = self.try_type_expression();
        self.skip_whitespace_or_asterisk();
        let name = self.declared_name();
        self.skip_whitespace();
        let has_comment = self.tag_comments(indent, None);
        let mut ty = written
            .clone()
            .map_or(JsDocTagType::None, JsDocTagType::Expr);
        if written.is_none() || self.is_object_or_object_array(&written) {
            let mut has_children = false;
            let mut child_type: Option<JsDocTypeExpr> = None;
            let mut properties = Vec::new();
            loop {
                let mark = self.mark();
                let Some(child) = self.child_tag(PROPERTY, indent, None) else {
                    self.rewind(&mark);
                    break;
                };
                has_children = true;
                match child.kind {
                    JsDocTagKind::Template(_) => {
                        self.error(child.name_pos as usize, "template".len(), 8039)
                    }
                    JsDocTagKind::Type(expr) if child_type.is_none() => child_type = Some(expr),
                    JsDocTagKind::Type(_) => self.error_at_token(8033),
                    _ => properties.push(child),
                }
            }
            if has_children {
                ty = match child_type {
                    Some(expr) if !self.is_object_or_object_array(&Some(expr.clone())) => {
                        JsDocTagType::Expr(expr)
                    }
                    _ => JsDocTagType::Literal {
                        pos: properties.first().map_or(start as u32, |first| first.pos),
                        is_array: self.is_array_type(&written),
                        properties,
                    },
                };
            }
        }
        if !has_comment {
            let end = (name.name.end.max(tag_name.end)) as usize;
            self.trailing_tag_comments(start, end, indent, indent_text);
        }
        JsDocTagKind::Typedef(JsDocTypedef { name, ty })
    }

    /// `parseJSDocSignature`
    fn signature(&mut self, pos: usize, indent: usize) -> JsDocSignature {
        // `parseCallbackTagParameters`
        let mut params = Vec::new();
        loop {
            let mark = self.mark();
            let Some(child) = self.child_tag(CALLBACK_PARAMETER, indent, None) else {
                self.rewind(&mark);
                break;
            };
            match child.kind {
                JsDocTagKind::Template(_) => {
                    self.error(child.name_pos as usize, "template".len(), 8039)
                }
                _ => params.push(child),
            }
        }
        let mut ret = None;
        let mut has_return_tag = false;
        let mark = self.mark();
        if self.eat_jsdoc(Token::At)
            && self.token == Token::At
            && let JsDocTagKind::Return(ty) = self.tag(&[], indent).kind
        {
            has_return_tag = true;
            ret = ty;
        }
        if !has_return_tag {
            self.rewind(&mark);
        }
        JsDocSignature {
            params,
            ret,
            pos: pos as u32,
        }
    }

    /// `parseCallbackTag`
    fn callback_tag(&mut self, start: usize, indent: usize, indent_text: usize) -> JsDocTagKind {
        let name = self.declared_name();
        self.skip_whitespace();
        let has_comment = self.tag_comments(indent, None);
        let pos = self.full_start();
        let signature = self.signature(pos, indent);
        if !has_comment {
            self.trailing_comments(start, indent, indent_text);
        }
        JsDocTagKind::Callback(JsDocCallback { name, signature })
    }

    /// `parseOverloadTag`
    fn overload_tag(&mut self, start: usize, indent: usize, indent_text: usize) -> JsDocTagKind {
        self.skip_whitespace();
        let has_comment = self.tag_comments(indent, None);
        let signature = self.signature(start, indent);
        if !has_comment {
            self.trailing_comments(start, indent, indent_text);
        }
        JsDocTagKind::Overload(signature)
    }

    /// `parseChildParameterOrPropertyTag`. `name`: the name of the parent tag the child must belong
    /// to.
    fn child_tag(
        &mut self,
        target: u8,
        indent: usize,
        name: Option<&[JsDocName]>,
    ) -> Option<JsDocTag> {
        let mut can_parse_tag = true;
        let mut seen_asterisk = false;
        loop {
            match self.next_jsdoc() {
                Token::At => {
                    if can_parse_tag && self.can_follow_at() {
                        let child = self.try_child_tag(target, indent)?;
                        if let Some(name) = name
                            && let JsDocTagKind::Param(property) | JsDocTagKind::Property(property) =
                                &child.kind
                            && !is_part_of(&property.name, name)
                        {
                            return None;
                        }
                        return Some(child);
                    }
                    seen_asterisk = false;
                }
                Token::NewLine => {
                    can_parse_tag = true;
                    seen_asterisk = false;
                }
                Token::Asterisk => {
                    if seen_asterisk {
                        can_parse_tag = false;
                    }
                    seen_asterisk = true;
                }
                Token::Word => can_parse_tag = false,
                Token::EndOfFile => return None,
                _ => {}
            }
        }
    }

    /// `tryParseChildTag`
    fn try_child_tag(&mut self, target: u8, indent: usize) -> Option<JsDocTag> {
        let start = self.start;
        self.next_jsdoc();
        let name = self.identifier_name(Some(1003));
        let indent_text = self.skip_whitespace_or_asterisk();
        let fits = match name.text.as_str() {
            "type" if target == PROPERTY => {
                return Some(self.type_tag(&[], start, name, None, 0));
            }
            "prop" | "property" => PROPERTY,
            "arg" | "argument" | "param" => PARAMETER | CALLBACK_PARAMETER,
            "template" => return Some(self.template_tag(start, name, indent, indent_text)),
            "this" => return Some(self.this_tag(start, name, indent, indent_text)),
            _ => return None,
        };
        if target & fits == 0 {
            return None;
        }
        Some(self.parameter_or_property_tag(start, name, target, indent))
    }

    /// `parseModifiersEx`, before the name of a type parameter.
    fn type_parameter_modifiers(&mut self) -> Vec<(JsDocTypeParameterModifier, u32)> {
        let mut modifiers = Vec::new();
        while self.token == Token::Word {
            // `IsModifierKind`. Nothing that can follow `export` or `default` is a type parameter.
            let (flag, is_listed) = match self.token_text() {
                "const" => (Some(JsDocTypeParameterModifier::Const), true),
                "in" => (Some(JsDocTypeParameterModifier::In), true),
                "out" => (Some(JsDocTypeParameterModifier::Out), true),
                "abstract" | "accessor" | "async" | "declare" | "default" | "export"
                | "override" | "private" | "protected" | "public" | "readonly" | "static" => {
                    (None, true)
                }
                _ => (None, false),
            };
            if !is_listed {
                break;
            }
            // `nextTokenCanFollowModifier`
            let (mark, pos) = (self.mark(), self.start as u32);
            self.next_token();
            let can_follow = self.is_in_lexer
                && matches!(
                    self.types.s.tok,
                    Tok::Ident
                        | Tok::PrivateIdent
                        | Tok::OpenBracket
                        | Tok::OpenBrace
                        | Tok::Star
                        | Tok::DotDotDot
                        | Tok::Str
                        | Tok::Number
                        | Tok::BigInt
                );
            if !can_follow || self.has_newline_before {
                self.rewind(&mark);
                break;
            }
            if let Some(flag) = flag {
                modifiers.push((flag, pos));
            }
        }
        modifiers
    }

    /// `parseTemplateTagTypeParameter`
    fn template_type_parameter(&mut self) -> Option<JsDocTypeParameter> {
        let pos = self.start as u32;
        let is_bracketed = self.eat_jsdoc(Token::OpenBracket);
        if is_bracketed {
            self.skip_whitespace();
        }
        let modifiers = self.type_parameter_modifiers();
        let name = self.identifier_name(Some(1069));
        let mut default = None;
        if is_bracketed {
            self.skip_whitespace();
            self.expect(Token::Equals);
            default = Some(self.jsdoc_type());
            self.expect(Token::CloseBracket);
        }
        (!name.is_missing()).then(|| JsDocTypeParameter {
            pos,
            name,
            modifiers,
            default,
            end: self.full_start() as u32,
        })
    }

    /// `parseTemplateTag`
    fn template_tag(
        &mut self,
        start: usize,
        name: JsDocName,
        indent: usize,
        indent_text: usize,
    ) -> JsDocTag {
        let constraint = (self.token == Token::OpenBrace).then(|| self.type_expression(false));
        // `parseTemplateTagTypeParameters`
        let mut params = Vec::new();
        loop {
            self.skip_whitespace();
            params.extend(self.template_type_parameter());
            self.skip_whitespace_or_asterisk();
            if !self.eat_jsdoc(Token::Comma) {
                break;
            }
        }
        self.trailing_comments(start, indent, indent_text);
        JsDocTag {
            kind: JsDocTagKind::Template(JsDocTemplate { constraint, params }),
            pos: start as u32,
            name_pos: name.start,
            end: self.full_start() as u32,
        }
    }
}

/// `isObjectOrObjectArrayTypeReference`
fn is_object_or_object_array(ty: &TypeExpr) -> bool {
    match &ty.kind {
        TypeExprKind::Keyword(TypeKeyword::Object) => true,
        TypeExprKind::Array(element) => is_object_or_object_array(element),
        TypeExprKind::Reference { name, arguments } => {
            arguments.is_empty() && name.len() == 1 && name[0] == "Object"
        }
        _ => false,
    }
}

/// `textsEqual` of `parent` and the part of `child` before its last dot.
fn is_part_of(child: &[JsDocName], parent: &[JsDocName]) -> bool {
    child.len() == parent.len() + 1 && child.iter().zip(parent).all(|(a, b)| a.text == b.text)
}

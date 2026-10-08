//! The scanner of the types inside a JSDoc comment: TypeScript's `scanner.go`, over the text of a
//! comment instead of a file. The tags around the types are read by [`super::tags`] with its own
//! scanner (`ScanJSDocToken`).

use oxc_syntax::identifier::{is_identifier_part, is_identifier_start};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Tok {
    Eof,
    /// An identifier or a keyword. [`Scanner::value`] is its text.
    Ident,
    PrivateIdent,
    Number,
    BigInt,
    Str,
    NoSubstitutionTemplate,
    TemplateHead,
    TemplateMiddle,
    TemplateTail,
    OpenBrace,
    CloseBrace,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    Lt,
    Gt,
    Comma,
    Colon,
    Semi,
    Dot,
    DotDotDot,
    Question,
    Excl,
    Star,
    Eq,
    Arrow,
    Bar,
    Amp,
    At,
    Minus,
    Plus,
    Slash,
    Backtick,
    Unknown,
}

/// Why a token is wrong, as the scanner reports it: the code, and the range.
#[derive(Clone, Debug)]
pub(super) struct ScanError {
    pub code: u32,
    pub start: usize,
    pub len: usize,
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pos: usize,
    tok: Tok,
    full_start: usize,
    start: usize,
    end: usize,
    newline_before: bool,
    value_len: usize,
    value: String,
    cooked: Vec<u16>,
    asterisks: usize,
    errors: usize,
    unterminated: bool,
    has_escape: bool,
}

pub(super) struct Scanner<'a> {
    pub text: &'a str,
    /// Leading asterisks of the lines of a comment are trivia.
    pub skip_asterisks: bool,
    pub asterisks: Vec<u32>,
    pub errors: Vec<ScanError>,
    pos: usize,
    pub tok: Tok,
    pub full_start: usize,
    pub start: usize,
    pub end: usize,
    pub newline_before: bool,
    /// The decoded text of an identifier, or the text of a number.
    pub value: String,
    /// The cooked value of a string or a piece of a template, in UTF-16 units.
    pub cooked: Vec<u16>,
    pub unterminated: bool,
    /// An identifier spelled with an escape.
    pub has_escape: bool,
    value_len: usize,
}

fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\u{0B}' | '\u{0C}' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

impl<'a> Scanner<'a> {
    pub fn new(text: &'a str, pos: usize) -> Self {
        Scanner {
            text,
            skip_asterisks: false,
            asterisks: Vec::new(),
            errors: Vec::new(),
            pos,
            tok: Tok::Unknown,
            full_start: pos,
            start: pos,
            end: pos,
            newline_before: false,
            value: String::new(),
            cooked: Vec::new(),
            unterminated: false,
            has_escape: false,
            value_len: 0,
        }
    }

    /// The next token is scanned from `pos`.
    pub fn reset_to(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// The current identifier ends at `end` instead, and is `value`.
    pub fn set_token_end(&mut self, end: usize, value: String) {
        self.pos = end;
        self.end = end;
        self.value = value;
        self.tok = Tok::Ident;
    }

    /// The cooked value of the string at the current token.
    pub fn cooked_string(&self) -> String {
        String::from_utf16_lossy(&self.cooked)
    }

    /// Where the next token is scanned from.
    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            pos: self.pos,
            tok: self.tok,
            full_start: self.full_start,
            start: self.start,
            end: self.end,
            newline_before: self.newline_before,
            value_len: self.value_len,
            value: self.value.clone(),
            cooked: self.cooked.clone(),
            asterisks: self.asterisks.len(),
            errors: self.errors.len(),
            unterminated: self.unterminated,
            has_escape: self.has_escape,
        }
    }

    pub fn restore(&mut self, snapshot: &Snapshot) {
        self.pos = snapshot.pos;
        self.tok = snapshot.tok;
        self.full_start = snapshot.full_start;
        self.start = snapshot.start;
        self.end = snapshot.end;
        self.newline_before = snapshot.newline_before;
        self.value_len = snapshot.value_len;
        self.value.clone_from(&snapshot.value);
        self.cooked.clone_from(&snapshot.cooked);
        self.asterisks.truncate(snapshot.asterisks);
        self.errors.truncate(snapshot.errors);
        self.unterminated = snapshot.unterminated;
        self.has_escape = snapshot.has_escape;
    }

    /// The text of the current token.
    pub fn token_text(&self) -> &'a str {
        &self.text[self.start.min(self.end)..self.end]
    }

    fn peek(&self, at: usize) -> Option<char> {
        self.text.get(at..)?.chars().next()
    }

    fn error(&mut self, code: u32, start: usize, len: usize) {
        self.errors.push(ScanError { code, start, len });
    }

    /// Skips trivia, and scans the next token.
    pub fn next(&mut self) -> Tok {
        self.full_start = self.pos;
        self.newline_before = false;
        self.unterminated = false;
        self.has_escape = false;
        let mut saw_asterisk = false;
        loop {
            let Some(c) = self.peek(self.pos) else {
                self.start = self.text.len();
                self.end = self.text.len();
                self.pos = self.text.len();
                self.tok = Tok::Eof;
                return self.tok;
            };
            if is_line_break(c) {
                self.newline_before = true;
                saw_asterisk = false;
                self.pos += c.len_utf8();
                if c == '\r' && self.peek(self.pos) == Some('\n') {
                    self.pos += 1;
                }
            } else if is_space(c) {
                self.pos += c.len_utf8();
            } else if c == '*'
                && self.skip_asterisks
                && self.newline_before
                && !saw_asterisk
                && self.peek(self.pos + 1) != Some('/')
            {
                // `TokenFlagsPrecedingJSDocLeadingAsterisks`
                saw_asterisk = true;
                self.asterisks.push(self.pos as u32);
                self.pos += 1;
            } else if c == '/' && self.peek(self.pos + 1) == Some('/') {
                while let Some(c) = self.peek(self.pos) {
                    if is_line_break(c) {
                        break;
                    }
                    self.pos += c.len_utf8();
                }
            } else if c == '/' && self.peek(self.pos + 1) == Some('*') {
                self.pos += 2;
                loop {
                    match self.peek(self.pos) {
                        None => break,
                        Some('*') if self.peek(self.pos + 1) == Some('/') => {
                            self.pos += 2;
                            break;
                        }
                        Some(c) => {
                            if is_line_break(c) {
                                self.newline_before = true;
                            }
                            self.pos += c.len_utf8();
                        }
                    }
                }
            } else {
                break;
            }
        }
        self.scan_token()
    }

    fn scan_token(&mut self) -> Tok {
        let start = self.pos;
        self.start = start;
        let c = self.peek(start).unwrap_or('\0');
        let single = |this: &mut Self, tok| {
            this.pos = start + 1;
            this.end = this.pos;
            this.tok = tok;
            tok
        };
        match c {
            '{' => single(self, Tok::OpenBrace),
            '}' => single(self, Tok::CloseBrace),
            '(' => single(self, Tok::OpenParen),
            ')' => single(self, Tok::CloseParen),
            '[' => single(self, Tok::OpenBracket),
            ']' => single(self, Tok::CloseBracket),
            '<' => single(self, Tok::Lt),
            '>' => single(self, Tok::Gt),
            ',' => single(self, Tok::Comma),
            ':' => single(self, Tok::Colon),
            ';' => single(self, Tok::Semi),
            '?' => single(self, Tok::Question),
            '!' => single(self, Tok::Excl),
            '*' => single(self, Tok::Star),
            '|' => single(self, Tok::Bar),
            '&' => single(self, Tok::Amp),
            '@' => single(self, Tok::At),
            '-' => single(self, Tok::Minus),
            '+' => single(self, Tok::Plus),
            '/' => single(self, Tok::Slash),
            '=' => {
                if self.peek(start + 1) == Some('>') {
                    self.pos = start + 2;
                    self.end = self.pos;
                    self.tok = Tok::Arrow;
                    Tok::Arrow
                } else {
                    single(self, Tok::Eq)
                }
            }
            '.' => {
                if self.text[start..].starts_with("...") {
                    self.pos = start + 3;
                    self.end = self.pos;
                    self.tok = Tok::DotDotDot;
                    Tok::DotDotDot
                } else if self.peek(start + 1).is_some_and(|c| c.is_ascii_digit()) {
                    self.scan_number()
                } else {
                    single(self, Tok::Dot)
                }
            }
            '0'..='9' => self.scan_number(),
            '"' | '\'' => self.scan_string(c),
            '`' => self.scan_template(true),
            '#' => {
                // A private name.
                let name_start = start + 1;
                match self.peek(name_start) {
                    Some(c) if is_identifier_start(c) || c == '\\' => {
                        self.scan_identifier_into(name_start);
                        self.value.insert(0, '#');
                        self.tok = Tok::PrivateIdent;
                        Tok::PrivateIdent
                    }
                    _ => single(self, Tok::Unknown),
                }
            }
            '\\' => {
                if self.text[start..].starts_with("\\u") && self.escape_at(start).is_some() {
                    self.scan_identifier_into(start);
                    self.tok = Tok::Ident;
                    Tok::Ident
                } else {
                    single(self, Tok::Unknown)
                }
            }
            c if is_identifier_start(c) => {
                self.scan_identifier_into(start);
                self.tok = Tok::Ident;
                Tok::Ident
            }
            c => {
                self.pos = start + c.len_utf8();
                self.end = self.pos;
                self.tok = Tok::Unknown;
                Tok::Unknown
            }
        }
    }

    /// The character of the unicode escape `\uXXXX` or `\u{X..}` at `at`, and its length.
    fn escape_at(&self, at: usize) -> Option<(char, usize)> {
        let rest = self.text.get(at..)?;
        let rest = rest.strip_prefix("\\u")?;
        if let Some(braced) = rest.strip_prefix('{') {
            let close = braced.find('}')?;
            let code = u32::from_str_radix(&braced[..close], 16).ok()?;
            return Some((char::from_u32(code)?, 3 + close + 1));
        }
        let hex = rest.get(..4)?;
        let code = u32::from_str_radix(hex, 16).ok()?;
        Some((char::from_u32(code)?, 6))
    }

    /// Scans the identifier that starts at `from`, decoding its escapes.
    fn scan_identifier_into(&mut self, from: usize) {
        self.value.clear();
        let mut at = from;
        while let Some(c) = self.peek(at) {
            if c == '\\' {
                match self.escape_at(at) {
                    Some((decoded, len))
                        if (at == from && is_identifier_start(decoded))
                            || (at > from && is_identifier_part(decoded)) =>
                    {
                        self.value.push(decoded);
                        self.has_escape = true;
                        at += len;
                        continue;
                    }
                    _ => break,
                }
            }
            if (at == from && is_identifier_start(c)) || (at > from && is_identifier_part(c)) {
                self.value.push(c);
                at += c.len_utf8();
            } else {
                break;
            }
        }
        self.pos = at.max(from + 1);
        self.end = self.pos;
    }

    fn scan_number(&mut self) -> Tok {
        let start = self.pos;
        let bytes = self.text.as_bytes();
        let mut at = start;
        let digits = |at: &mut usize, accept: fn(u8) -> bool| {
            while *at < bytes.len() && (accept(bytes[*at]) || bytes[*at] == b'_') {
                *at += 1;
            }
        };
        let mut tok = Tok::Number;
        let radix_prefix = bytes[at] == b'0'
            && matches!(
                bytes.get(at + 1),
                Some(b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
            );
        if radix_prefix {
            at += 2;
            digits(&mut at, |b| b.is_ascii_hexdigit());
        } else {
            digits(&mut at, |b| b.is_ascii_digit());
            if bytes.get(at) == Some(&b'.') {
                at += 1;
                digits(&mut at, |b| b.is_ascii_digit());
            }
            if matches!(bytes.get(at), Some(b'e' | b'E')) {
                let mut probe = at + 1;
                if matches!(bytes.get(probe), Some(b'+' | b'-')) {
                    probe += 1;
                }
                if bytes.get(probe).is_some_and(u8::is_ascii_digit) {
                    at = probe;
                    digits(&mut at, |b| b.is_ascii_digit());
                }
            }
        }
        if bytes.get(at) == Some(&b'n') {
            tok = Tok::BigInt;
        }
        self.value = self.text[start..at].replace('_', "");
        if tok == Tok::BigInt {
            at += 1;
        }
        self.pos = at;
        self.end = at;
        self.tok = tok;
        tok
    }

    fn scan_string(&mut self, quote: char) -> Tok {
        let start = self.pos;
        let mut at = start + 1;
        self.cooked.clear();
        loop {
            match self.peek(at) {
                None => {
                    self.unterminated = true;
                    self.error(1002, start, at - start);
                    break;
                }
                Some(c) if c == quote => {
                    at += 1;
                    break;
                }
                Some('\\') => {
                    at = self.scan_escape(at);
                }
                Some(c) if c == '\n' || c == '\r' => {
                    self.unterminated = true;
                    self.error(1002, start, at - start);
                    break;
                }
                Some(c) => {
                    let mut buf = [0u16; 2];
                    self.cooked.extend_from_slice(c.encode_utf16(&mut buf));
                    at += c.len_utf8();
                }
            }
        }
        self.pos = at;
        self.end = at;
        self.tok = Tok::Str;
        Tok::Str
    }

    /// The escape sequence at `at`, which is a backslash: appends its value to `cooked` and
    /// returns the position after it.
    fn scan_escape(&mut self, at: usize) -> usize {
        let Some(c) = self.peek(at + 1) else {
            return at + 1;
        };
        let after = at + 1 + c.len_utf8();
        let mut push = |this: &mut Self, unit: u16| this.cooked.push(unit);
        match c {
            'n' => push(self, 10),
            'r' => push(self, 13),
            't' => push(self, 9),
            'b' => push(self, 8),
            'f' => push(self, 12),
            'v' => push(self, 11),
            '0' if !self.peek(after).is_some_and(|c| c.is_ascii_digit()) => push(self, 0),
            'x' => {
                if let Some(hex) = self.text.get(after..after + 2)
                    && let Ok(code) = u16::from_str_radix(hex, 16)
                {
                    push(self, code);
                    return after + 2;
                }
            }
            'u' => {
                if let Some((decoded, len)) = self.escape_at(at) {
                    let mut buf = [0u16; 2];
                    self.cooked
                        .extend_from_slice(decoded.encode_utf16(&mut buf));
                    return at + len;
                }
                if let Some(hex) = self.text.get(after..after + 4)
                    && let Ok(code) = u16::from_str_radix(hex, 16)
                {
                    // A lone surrogate.
                    push(self, code);
                    return after + 4;
                }
            }
            '\r' => {
                if self.peek(after) == Some('\n') {
                    return after + 1;
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            c => {
                let mut buf = [0u16; 2];
                self.cooked.extend_from_slice(c.encode_utf16(&mut buf));
            }
        }
        after
    }

    /// A template from its backtick (`head`) or from the `}` that continues it.
    fn scan_template(&mut self, head: bool) -> Tok {
        let start = self.pos;
        let mut at = start + 1;
        self.cooked.clear();
        let tok = loop {
            match self.peek(at) {
                None => {
                    self.unterminated = true;
                    self.error(1160, start, at - start);
                    break if head {
                        Tok::NoSubstitutionTemplate
                    } else {
                        Tok::TemplateTail
                    };
                }
                Some('`') => {
                    at += 1;
                    break if head {
                        Tok::NoSubstitutionTemplate
                    } else {
                        Tok::TemplateTail
                    };
                }
                Some('$') if self.peek(at + 1) == Some('{') => {
                    at += 2;
                    break if head {
                        Tok::TemplateHead
                    } else {
                        Tok::TemplateMiddle
                    };
                }
                Some('\\') => at = self.scan_escape(at),
                Some(c) => {
                    let mut buf = [0u16; 2];
                    self.cooked.extend_from_slice(c.encode_utf16(&mut buf));
                    at += c.len_utf8();
                }
            }
        };
        self.pos = at;
        self.end = at;
        self.tok = tok;
        tok
    }

    /// The current token is a `}` that continues a template: scans the rest of the template.
    pub fn rescan_template_continuation(&mut self) -> Tok {
        self.pos = self.start;
        self.scan_template(false)
    }

    /// A word `isIdentifier` rejects (`tokenIsReservedWord`).
    pub fn is_reserved_word(word: &str) -> bool {
        matches!(
            word,
            "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "enum"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "new"
                | "null"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
        )
    }
}

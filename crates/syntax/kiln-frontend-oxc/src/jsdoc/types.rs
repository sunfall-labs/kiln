//! The types inside a JSDoc comment: the type grammar of TypeScript's `parser.go`, with the JSDoc
//! additions of its type parser (`?T`, `T?`, `!T`, `T!`, `*`, `A.<T>`).
//!
//! A port of the recursive descent, not of its tree: a node here is the owned [`TypeExpr`] the
//! rest of the frontend already produces for TypeScript, so a checker lowers the types of a
//! comment as it lowers those of a `.ts` file. Errors are reported and recovered from as
//! TypeScript does, because the baselines contain them.

use super::scan::{Scanner, Snapshot, Tok};
use kiln_hir::jsdoc::{
    JsDocDiagnostic, JsDocDiagnosticKind, JsDocImportSpecifier, JsDocName, JsDocResolutionMode,
};
use kiln_hir::{
    JsDocTypeKind, Span, StringUnits, TypeExpr, TypeExprKind, TypeKeyword, TypeLiteral, TypeMember,
    TypeMemberKind, TypeModifier, TypeOperator, TypeParameter, TypeSignature,
    TypeSignatureParameter, TypeTupleElement,
};

/// `ParsingContext`
#[derive(Copy, Clone, PartialEq, Eq)]
enum Ctx {
    TypeMembers,
    TupleElementTypes,
    TypeParameters,
    TypeArguments,
    Parameters,
    ImportAttributes,
    ImportSpecifiers,
}

pub(super) struct TypeParser<'a> {
    pub s: Scanner<'a>,
    pub diagnostics: Vec<JsDocDiagnostic>,
    contexts: Vec<Ctx>,
    disallow_conditional: bool,
    /// `inAwaitContext`, `inYieldContext`
    pub await_context: bool,
    pub yield_context: bool,
}

pub(super) struct Mark {
    scanner: Snapshot,
    diagnostics: usize,
    contexts: usize,
}

fn is_modifier(word: &str) -> bool {
    matches!(
        word,
        "abstract"
            | "accessor"
            | "async"
            | "const"
            | "declare"
            | "default"
            | "export"
            | "in"
            | "out"
            | "override"
            | "private"
            | "protected"
            | "public"
            | "readonly"
            | "static"
    )
}

impl<'a> TypeParser<'a> {
    /// A parser that reads the types in `text` from `pos`. The first token is scanned.
    pub fn new(text: &'a str, pos: usize, skip_asterisks: bool) -> Self {
        let mut s = Scanner::new(text, pos);
        s.skip_asterisks = skip_asterisks;
        s.next();
        TypeParser {
            s,
            diagnostics: Vec::new(),
            contexts: Vec::new(),
            disallow_conditional: false,
            await_context: false,
            yield_context: false,
        }
    }

    // ───────────────────────────── tokens ─────────────────────────────

    fn tok(&self) -> Tok {
        self.s.tok
    }

    pub fn next_token(&mut self) -> Tok {
        self.s.next()
    }

    /// The text of the current token, if it is an identifier or a keyword written without
    /// escapes.
    fn word(&self) -> Option<&str> {
        (self.tok() == Tok::Ident && !self.s.has_escape).then_some(self.s.value.as_str())
    }

    fn is_word(&self, word: &str) -> bool {
        self.word() == Some(word)
    }

    /// `isIdentifier`
    fn is_identifier(&self) -> bool {
        self.tok() == Tok::Ident
            && (self.s.has_escape || !Scanner::is_reserved_word(&self.s.value))
            && !(self.await_context && self.s.value == "await")
            && !(self.yield_context && self.s.value == "yield")
    }

    fn pos(&self) -> usize {
        self.s.start
    }

    /// Where the node being finished ends: after the last token it consumed.
    fn end(&self) -> usize {
        self.s.full_start
    }

    pub(super) fn mark(&self) -> Mark {
        Mark {
            scanner: self.s.snapshot(),
            diagnostics: self.diagnostics.len(),
            contexts: self.contexts.len(),
        }
    }

    pub(super) fn rewind(&mut self, mark: &Mark) {
        self.s.restore(&mark.scanner);
        self.diagnostics.truncate(mark.diagnostics);
        self.contexts.truncate(mark.contexts);
    }

    fn look_ahead<R>(&mut self, scan: impl FnOnce(&mut Self) -> R) -> R {
        let mark = self.mark();
        let result = scan(self);
        self.rewind(&mark);
        result
    }

    fn try_parse<R>(&mut self, parse: impl FnOnce(&mut Self) -> Option<R>) -> Option<R> {
        let mark = self.mark();
        let result = parse(self);
        if result.is_none() {
            self.rewind(&mark);
        }
        result
    }

    /// Makes the next token the one scanned from `pos`.
    pub fn reset_to(&mut self, pos: usize) {
        self.s.reset_to(pos);
    }

    pub fn error_at_current_token(&mut self, code: u32) {
        self.error_at_token(code, &[]);
    }

    pub fn is_word_token(&self, word: &str) -> bool {
        self.is_word(word)
    }

    pub fn is_identifier_token(&self) -> bool {
        self.is_identifier()
    }

    pub fn expect_token(&mut self, tok: Tok) -> bool {
        self.expect(tok)
    }

    /// `parseExpectedContextual`: the contextual keyword `word`.
    pub fn expect_contextual_keyword(&mut self, word: &str) -> bool {
        self.expect_word(word)
    }

    /// The identifier or string at the current token, as a name.
    pub fn name_at_token(&self) -> JsDocName {
        let text = if self.tok() == Tok::Str {
            StringUnits::from_units(&self.s.cooked).to_lossy_string()
        } else {
            self.s.value.clone()
        };
        JsDocName {
            start: self.s.start as u32,
            end: self.s.end as u32,
            text,
        }
    }

    // ───────────────────────────── imports ─────────────────────────────

    /// `parseNamedImports`: `{ a, type b, c as d }`, from the `{`.
    pub fn parse_named_imports(&mut self) -> Vec<JsDocImportSpecifier> {
        let mut specifiers = Vec::new();
        if self.expect(Tok::OpenBrace) {
            specifiers = self
                .parse_delimited_list(Ctx::ImportSpecifiers, false, Self::parse_import_specifier)
                .into_iter()
                .flatten()
                .collect();
            self.expect(Tok::CloseBrace);
        }
        specifiers
    }

    /// `parseModuleExportName(parseIdentifierName)`
    fn parse_module_export_name(&mut self) -> JsDocName {
        let name = self.name_at_token();
        if matches!(self.tok(), Tok::Ident | Tok::Str) {
            self.next_token();
            return name;
        }
        let (_, start, end) = self.missing_identifier(1003, &[]);
        JsDocName {
            start: start as u32,
            end: end as u32,
            text: String::new(),
        }
    }

    /// `parseNameWithKeywordCheck`
    fn parse_name_with_keyword_check(&mut self) -> JsDocName {
        if self.tok() == Tok::Ident && Scanner::is_reserved_word(&self.s.value) {
            let text = self.s.value.clone();
            self.error_at_token(1359, &[&text]);
        }
        let name = self.name_at_token();
        if self.tok() == Tok::Ident {
            self.next_token();
            return name;
        }
        let (_, start, end) = self.missing_identifier(1003, &[]);
        JsDocName {
            start: start as u32,
            end: end as u32,
            text: String::new(),
        }
    }

    /// `parseImportOrExportSpecifier`
    fn parse_import_specifier(&mut self) -> Option<Option<JsDocImportSpecifier>> {
        let start = self.pos();
        let mut is_type_only = false;
        let mut property_name: Option<JsDocName> = None;
        let mut can_parse_as_keyword = true;
        let first_is_identifier = self.tok() == Tok::Ident;
        let mut name = self.parse_module_export_name();
        if first_is_identifier && name.text == "type" {
            // import { type } from "mod";          - isTypeOnly: false,   name: type
            // import { type as } from "mod";       - isTypeOnly: true,    name: as
            // import { type as as } from "mod";    - isTypeOnly: false,   name: as,    propertyName: type
            // import { type as as as } from "mod"; - isTypeOnly: true,    name: as,    propertyName: as
            if self.is_word("as") {
                // { type as ...? }
                let first_as = self.parse_module_export_name();
                if self.is_word("as") {
                    // { type as as ...? }
                    let second_as = self.parse_module_export_name();
                    if self.tok() == Tok::Ident {
                        // { type as as something }
                        is_type_only = true;
                        property_name = Some(first_as);
                        name = self.parse_name_with_keyword_check();
                        can_parse_as_keyword = false;
                    } else {
                        // { type as as }
                        property_name = Some(name);
                        name = second_as;
                        can_parse_as_keyword = false;
                    }
                } else if self.tok() == Tok::Ident {
                    // { type as something }
                    property_name = Some(name);
                    can_parse_as_keyword = false;
                    name = self.parse_name_with_keyword_check();
                } else {
                    // { type as }
                    is_type_only = true;
                    name = first_as;
                }
            } else if self.tok() == Tok::Ident {
                // { type something ...? }
                is_type_only = true;
                name = self.parse_name_with_keyword_check();
            }
        }
        if can_parse_as_keyword && self.is_word("as") {
            property_name = Some(name);
            self.expect_word("as");
            name = self.parse_name_with_keyword_check();
        }
        let imported = property_name.clone().unwrap_or_else(|| name.clone());
        // A string name is no binding.
        let is_string = self
            .s
            .text
            .as_bytes()
            .get(name.start as usize)
            .is_some_and(|&b| b == b'"' || b == b'\'');
        if name.text.is_empty() || is_string {
            return Some(None);
        }
        Some(Some(JsDocImportSpecifier {
            start: start as u32,
            is_type_only,
            imported: imported.text,
            imported_pos: imported.start,
            local: name.text,
            local_pos: name.start,
        }))
    }

    /// `tryParseImportAttributes`: `with { key: "value" }`, from the keyword. The resolution mode
    /// it names.
    pub fn parse_import_attributes(&mut self) -> JsDocResolutionMode {
        let mut mode = JsDocResolutionMode::None;
        self.next_token();
        if !self.expect(Tok::OpenBrace) {
            return mode;
        }
        let attributes = self.parse_delimited_list(Ctx::ImportAttributes, true, |this| {
            let key = this.name_at_token();
            this.next_token();
            this.expect(Tok::Colon);
            let value = if this.tok() == Tok::Str {
                let value = StringUnits::from_units(&this.s.cooked).to_lossy_string();
                this.next_token();
                Some(value)
            } else {
                this.error_at_token(1003, &[]);
                None
            };
            Some((key, value))
        });
        for (key, value) in attributes {
            if key.text == "resolution-mode" {
                mode = match value.as_deref() {
                    Some("import") => JsDocResolutionMode::Import,
                    Some("require") => JsDocResolutionMode::Require,
                    _ => mode,
                };
            }
        }
        self.expect(Tok::CloseBrace);
        mode
    }

    // ───────────────────────────── errors ─────────────────────────────

    /// `parseErrorAtPosition`: not another error at the position of the last.
    pub(super) fn error_at(&mut self, start: usize, len: usize, code: u32, args: &[&str]) {
        if self.diagnostics.last().is_some_and(|last| {
            last.kind == JsDocDiagnosticKind::Parse && last.start as usize == start
        }) {
            return;
        }
        self.diagnostics.push(JsDocDiagnostic {
            kind: JsDocDiagnosticKind::Parse,
            start: start as u32,
            end: (start + len) as u32,
            code,
            args: args.iter().map(|&a| a.to_owned()).collect(),
        });
    }

    fn error_at_token(&mut self, code: u32, args: &[&str]) {
        let (start, end) = (self.s.start, self.s.end);
        self.error_at(start, end.saturating_sub(start), code, args);
    }

    /// `parseExpected`
    fn expect(&mut self, tok: Tok) -> bool {
        if self.tok() == tok {
            self.next_token();
            return true;
        }
        self.expected(token_text(tok));
        false
    }

    pub(super) fn expected(&mut self, text: &str) {
        self.error_at_token(1005, &[text]);
    }

    fn parse_optional(&mut self, tok: Tok) -> bool {
        if self.tok() == tok {
            self.next_token();
            return true;
        }
        false
    }

    fn parse_optional_word(&mut self, word: &str) -> bool {
        if self.is_word(word) {
            self.next_token();
            return true;
        }
        false
    }

    fn expect_word(&mut self, word: &str) -> bool {
        if self.parse_optional_word(word) {
            return true;
        }
        self.expected(word);
        false
    }

    /// `canParseSemicolon`
    fn can_parse_semicolon(&self) -> bool {
        matches!(self.tok(), Tok::Semi | Tok::CloseBrace | Tok::Eof) || self.s.newline_before
    }

    /// `parseSemicolon`
    fn parse_semicolon(&mut self) -> bool {
        if self.can_parse_semicolon() {
            if self.tok() == Tok::Semi {
                self.next_token();
            }
            return true;
        }
        self.expect(Tok::Semi)
    }

    // ───────────────────────────── identifiers ─────────────────────────────

    /// `createMissingNode` for an identifier: the error, and an empty name.
    fn missing_identifier(&mut self, code: u32, args: &[&str]) -> (String, usize, usize) {
        if self.tok() == Tok::Eof {
            let at = self.s.full_start;
            self.error_at(at, 0, code, args);
        } else {
            self.error_at_token(code, args);
        }
        (String::new(), self.s.full_start, self.s.full_start)
    }

    /// `parseIdentifier`: the name, and where it starts and ends.
    fn parse_identifier(&mut self) -> (String, usize, usize) {
        if self.is_identifier() {
            let result = (self.s.value.clone(), self.s.start, self.s.end);
            self.next_token();
            return result;
        }
        let reserved = self.tok() == Tok::Ident && Scanner::is_reserved_word(&self.s.value);
        let text = self.s.token_text().to_owned();
        if reserved {
            self.missing_identifier(1359, &[&text])
        } else {
            self.missing_identifier(1003, &[])
        }
    }

    /// `parseIdentifierName` with `message`: an identifier or a keyword.
    fn parse_identifier_name(&mut self, message: Option<u32>) -> (String, usize, usize) {
        if self.tok() == Tok::Ident {
            let result = (self.s.value.clone(), self.s.start, self.s.end);
            self.next_token();
            return result;
        }
        self.missing_identifier(message.unwrap_or(1003), &[])
    }

    /// `parseEntityName(true, Type_expected)`: the names, each with its start. `A.<T>` ends the name
    /// before the `<`.
    fn parse_entity_name(&mut self, message: u32) -> Vec<(String, usize, usize)> {
        let mut names = vec![self.parse_identifier_name(Some(message))];
        while self.parse_optional(Tok::Dot) {
            if self.tok() == Tok::Lt {
                // The entity is part of a JSDoc-style generic.
                break;
            }
            // `parseRightSideOfDot`
            if self.tok() == Tok::Ident {
                names.push(self.parse_identifier_name(None));
            } else {
                let at = self.s.full_start;
                if self.tok() == Tok::PrivateIdent {
                    self.error_at_token(18019, &[]);
                    self.next_token();
                } else {
                    self.missing_identifier(1003, &[]);
                }
                names.push((String::new(), at, at));
            }
        }
        names
    }

    // ───────────────────────────── nodes ─────────────────────────────

    fn finish(&self, start: usize, kind: TypeExprKind) -> TypeExpr {
        TypeExpr {
            span: Span::new(start as u32, self.end().max(start) as u32),
            kind,
        }
    }

    fn keyword(&self, start: usize, keyword: TypeKeyword) -> TypeExpr {
        self.finish(start, TypeExprKind::Keyword(keyword))
    }

    // ───────────────────────────── lists ─────────────────────────────

    /// `isListTerminator`
    fn is_list_terminator(&self, kind: Ctx) -> bool {
        if self.tok() == Tok::Eof {
            return true;
        }
        match kind {
            Ctx::TypeMembers | Ctx::ImportAttributes | Ctx::ImportSpecifiers => {
                self.tok() == Tok::CloseBrace
            }
            Ctx::TupleElementTypes => self.tok() == Tok::CloseBracket,
            Ctx::TypeParameters => {
                matches!(self.tok(), Tok::Gt | Tok::OpenParen | Tok::OpenBrace)
                    || self.is_word("extends")
                    || self.is_word("implements")
            }
            Ctx::TypeArguments => self.tok() != Tok::Comma,
            Ctx::Parameters => matches!(self.tok(), Tok::CloseParen | Tok::CloseBracket),
        }
    }

    /// `isListElement`
    fn is_list_element(&mut self, kind: Ctx) -> bool {
        match kind {
            Ctx::TypeMembers => self.look_ahead(Self::is_type_member_start),
            Ctx::TupleElementTypes => {
                self.tok() == Tok::OpenBracket || self.is_start_of_type(false)
            }
            Ctx::TypeParameters => {
                self.is_identifier()
                    || self
                        .word()
                        .is_some_and(|word| matches!(word, "const" | "in" | "out"))
            }
            Ctx::TypeArguments => self.tok() == Tok::Comma || self.is_start_of_type(false),
            Ctx::Parameters => self.is_start_of_parameter(false),
            Ctx::ImportAttributes | Ctx::ImportSpecifiers => {
                matches!(self.tok(), Tok::Ident | Tok::Str)
            }
        }
    }

    /// `abortParsingListOrMoveToNextToken`
    fn abort_parsing_list_or_move_to_next_token(&mut self, kind: Ctx) -> bool {
        match kind {
            Ctx::TypeMembers => self.error_at_token(1131, &[]),
            Ctx::TupleElementTypes => self.error_at_token(1110, &[]),
            Ctx::TypeParameters => self.error_at_token(1139, &[]),
            Ctx::TypeArguments => self.error_at_token(1099, &[]),
            Ctx::Parameters => self.error_at_token(1138, &[]),
            Ctx::ImportAttributes | Ctx::ImportSpecifiers => self.error_at_token(1003, &[]),
        }
        if self.is_in_some_parsing_context() {
            return true;
        }
        self.next_token();
        false
    }

    /// `isInSomeParsingContext`
    fn is_in_some_parsing_context(&mut self) -> bool {
        for index in (0..self.contexts.len()).rev() {
            let kind = self.contexts[index];
            if self.is_list_element(kind) || self.is_list_terminator(kind) {
                return true;
            }
        }
        false
    }

    /// `parseList`
    fn parse_list<T>(
        &mut self,
        kind: Ctx,
        mut parse: impl FnMut(&mut Self) -> Option<T>,
    ) -> Vec<T> {
        self.contexts.push(kind);
        let mut list = Vec::new();
        while !self.is_list_terminator(kind) {
            if self.is_list_element(kind) {
                list.extend(parse(self));
                continue;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.contexts.pop();
        list
    }

    /// `parseDelimitedList`
    fn parse_delimited_list<T>(
        &mut self,
        kind: Ctx,
        consider_semicolon_as_delimiter: bool,
        mut parse: impl FnMut(&mut Self) -> Option<T>,
    ) -> Vec<T> {
        self.contexts.push(kind);
        let mut list = Vec::new();
        loop {
            if self.is_list_element(kind) {
                let start = self.s.full_start;
                list.extend(parse(self));
                if self.parse_optional(Tok::Comma) {
                    continue;
                }
                if self.is_list_terminator(kind) {
                    break;
                }
                if kind == Ctx::ImportAttributes {
                    self.expected("',' or '}'");
                    // The message of TypeScript is "',' or '}' expected."; see `expected`.
                } else {
                    self.expected(",");
                }
                if consider_semicolon_as_delimiter
                    && self.tok() == Tok::Semi
                    && !self.s.newline_before
                {
                    self.next_token();
                }
                if start == self.s.full_start {
                    self.next_token();
                }
                continue;
            }
            if self.is_list_terminator(kind) {
                break;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.contexts.pop();
        list
    }

    /// `parseBracketedList`. `None`: the opening token is missing.
    fn parse_bracketed_list<T>(
        &mut self,
        kind: Ctx,
        open: Tok,
        close: Tok,
        parse: impl FnMut(&mut Self) -> Option<T>,
    ) -> Vec<T> {
        if self.expect(open) {
            let list = self.parse_delimited_list(kind, false, parse);
            self.expect(close);
            return list;
        }
        Vec::new()
    }

    // ───────────────────────────── types ─────────────────────────────

    /// `parseTypeOrTypePredicate`
    pub fn parse_type_or_predicate(&mut self) -> TypeExpr {
        let start = self.pos();
        let variable = if self.is_identifier() {
            self.try_parse(|this| {
                // `parseTypePredicatePrefix`
                let id = this.parse_identifier();
                if this.is_word("is") && !this.s.newline_before {
                    this.next_token();
                    return Some(id);
                }
                None
            })
        } else {
            None
        };
        let ty = self.parse_type();
        match variable {
            Some((name, _, _)) => self.finish(
                start,
                TypeExprKind::Predicate {
                    parameter: name,
                    asserts: false,
                    ty: Some(Box::new(ty)),
                },
            ),
            None => ty,
        }
    }

    /// `parseType`
    pub fn parse_type(&mut self) -> TypeExpr {
        if self.is_start_of_function_or_constructor_type() {
            return self.parse_function_or_constructor_type();
        }
        let start = self.pos();
        let ty = self.parse_union_type_or_higher();
        if !self.disallow_conditional
            && !self.s.newline_before
            && self.parse_optional_word("extends")
        {
            // The type following `extends` is not permitted to be another conditional type.
            let outer = std::mem::replace(&mut self.disallow_conditional, true);
            let extends = self.parse_type();
            self.disallow_conditional = outer;
            self.expect(Tok::Question);
            let outer = std::mem::replace(&mut self.disallow_conditional, false);
            let when_true = self.parse_type();
            self.expect(Tok::Colon);
            let when_false = self.parse_type();
            self.disallow_conditional = outer;
            return self.finish(
                start,
                TypeExprKind::Conditional {
                    check: Box::new(ty),
                    extends: Box::new(extends),
                    when_true: Box::new(when_true),
                    when_false: Box::new(when_false),
                },
            );
        }
        ty
    }

    /// `allowConditionalTypesAnd`
    fn parse_type_allowing_conditional(&mut self) -> TypeExpr {
        let outer = std::mem::replace(&mut self.disallow_conditional, false);
        let ty = self.parse_type_or_predicate();
        self.disallow_conditional = outer;
        ty
    }

    /// `isStartOfFunctionTypeOrConstructorType`
    fn is_start_of_function_or_constructor_type(&mut self) -> bool {
        if self.tok() == Tok::Lt {
            return true;
        }
        if self.tok() == Tok::OpenParen
            && self.look_ahead(Self::is_unambiguously_start_of_function_type)
        {
            return true;
        }
        self.is_word("new")
            || (self.is_word("abstract")
                && self.look_ahead(|this| {
                    this.next_token();
                    this.is_word("new")
                }))
    }

    /// `isUnambiguouslyStartOfFunctionType`
    fn is_unambiguously_start_of_function_type(&mut self) -> bool {
        self.next_token();
        if matches!(self.tok(), Tok::CloseParen | Tok::DotDotDot) {
            // `( )`, `( ...`
            return true;
        }
        if self.skip_parameter_start() {
            // We successfully skipped modifiers (if any) and an identifier or binding pattern.
            if matches!(
                self.tok(),
                Tok::Colon | Tok::Comma | Tok::Question | Tok::Eq
            ) {
                return true;
            }
            if self.tok() == Tok::CloseParen {
                self.next_token();
                if self.tok() == Tok::Arrow {
                    // `( xxx ) =>`
                    return true;
                }
            }
        }
        false
    }

    /// `skipParameterStart`
    fn skip_parameter_start(&mut self) -> bool {
        while self.word().is_some_and(is_modifier) {
            self.next_token();
        }
        if self.is_identifier() || self.is_word("this") {
            self.next_token();
            return true;
        }
        if matches!(self.tok(), Tok::OpenBracket | Tok::OpenBrace) {
            let before = self.diagnostics.len();
            self.skip_binding_pattern();
            return before == self.diagnostics.len();
        }
        false
    }

    /// A binding pattern, which no part of a type reads: its brackets and what is between them.
    fn skip_binding_pattern(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.tok() {
                Tok::OpenBracket | Tok::OpenBrace => depth += 1,
                Tok::CloseBracket | Tok::CloseBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        self.next_token();
                        return;
                    }
                }
                Tok::Eof => {
                    self.expected("]");
                    return;
                }
                _ => {}
            }
            self.next_token();
        }
    }

    /// `isStartOfParameter`
    fn is_start_of_parameter(&mut self, is_javascript_file: bool) -> bool {
        let _ = is_javascript_file;
        self.tok() == Tok::DotDotDot
            || self.is_identifier()
            || self.word().is_some_and(is_modifier)
            || self.is_word("this")
            || matches!(self.tok(), Tok::OpenBracket | Tok::OpenBrace)
            || self.is_start_of_type(true)
    }

    /// `isStartOfType`
    fn is_start_of_type(&mut self, in_start_of_parameter: bool) -> bool {
        match self.tok() {
            Tok::OpenBrace
            | Tok::OpenBracket
            | Tok::Lt
            | Tok::Bar
            | Tok::Amp
            | Tok::Str
            | Tok::Number
            | Tok::BigInt
            | Tok::Star
            | Tok::Question
            | Tok::Excl
            | Tok::DotDotDot
            | Tok::NoSubstitutionTemplate
            | Tok::TemplateHead => true,
            Tok::Minus => {
                !in_start_of_parameter
                    && self.look_ahead(|this| {
                        this.next_token();
                        matches!(this.tok(), Tok::Number | Tok::BigInt)
                    })
            }
            Tok::OpenParen => {
                !in_start_of_parameter
                    && self.look_ahead(Self::is_start_of_parenthesized_or_function_type)
            }
            Tok::Ident => match self.word() {
                Some(
                    "any" | "unknown" | "string" | "number" | "bigint" | "boolean" | "readonly"
                    | "symbol" | "unique" | "void" | "undefined" | "null" | "this" | "typeof"
                    | "never" | "new" | "true" | "false" | "object" | "infer" | "import"
                    | "asserts",
                ) => true,
                Some("function") => !in_start_of_parameter,
                _ => self.is_identifier(),
            },
            _ => false,
        }
    }

    fn is_start_of_parenthesized_or_function_type(&mut self) -> bool {
        self.next_token();
        self.tok() == Tok::CloseParen
            || self.is_start_of_parameter(false)
            || self.is_start_of_type(false)
    }

    /// `parseFunctionOrConstructorType`
    fn parse_function_or_constructor_type(&mut self) -> TypeExpr {
        let start = self.pos();
        if self.is_word("abstract") {
            self.next_token();
        }
        let is_constructor = self.parse_optional_word("new");
        let type_parameters = self.parse_type_parameters();
        let (parameters, this_parameter) = self.parse_parameters();
        let return_type = self.parse_return_type(Tok::Arrow, false);
        let rest = parameters.iter().any(|p| p.rest);
        let signature = TypeSignature {
            span: Span::new(start as u32, self.end() as u32),
            type_parameters,
            this_parameter: this_parameter.map(Box::new),
            parameters,
            rest,
            return_type: return_type.map(Box::new),
        };
        self.finish(
            start,
            if is_constructor {
                TypeExprKind::Constructor(signature)
            } else {
                TypeExprKind::Function(signature)
            },
        )
    }

    /// `parseReturnType`
    fn parse_return_type(&mut self, return_token: Tok, is_type: bool) -> Option<TypeExpr> {
        // `shouldParseReturnType`
        let parses = if return_token == Tok::Arrow {
            self.expect(return_token);
            true
        } else if self.parse_optional(Tok::Colon) {
            true
        } else if is_type && self.tok() == Tok::Arrow {
            self.expected(":");
            self.next_token();
            true
        } else {
            false
        };
        parses.then(|| self.parse_type_allowing_conditional())
    }

    /// `parseUnionTypeOrHigher`
    fn parse_union_type_or_higher(&mut self) -> TypeExpr {
        self.parse_union_or_intersection(Tok::Bar)
    }

    fn parse_union_or_intersection(&mut self, operator: Tok) -> TypeExpr {
        let start = self.pos();
        let is_union = operator == Tok::Bar;
        let has_leading_operator = self.parse_optional(operator);
        let parse_constituent = |this: &mut Self| {
            if is_union {
                this.parse_intersection_type_or_higher()
            } else {
                this.parse_type_operator_or_higher()
            }
        };
        let mut ty = if has_leading_operator {
            self.parse_function_or_constructor_type_to_error(is_union)
                .unwrap_or_else(|| parse_constituent(self))
        } else {
            parse_constituent(self)
        };
        if self.tok() == operator || has_leading_operator {
            let mut types = vec![ty];
            while self.parse_optional(operator) {
                let next = self
                    .parse_function_or_constructor_type_to_error(is_union)
                    .unwrap_or_else(|| parse_constituent(self));
                types.push(next);
            }
            ty = self.finish(
                start,
                if is_union {
                    TypeExprKind::Union(types)
                } else {
                    TypeExprKind::Intersection(types)
                },
            );
        }
        ty
    }

    fn parse_intersection_type_or_higher(&mut self) -> TypeExpr {
        self.parse_union_or_intersection(Tok::Amp)
    }

    /// `parseFunctionOrConstructorTypeToError`
    fn parse_function_or_constructor_type_to_error(
        &mut self,
        is_in_union: bool,
    ) -> Option<TypeExpr> {
        // The function type after `|` or `&` needs parentheses.
        if self.is_start_of_function_or_constructor_type() {
            let ty = self.parse_function_or_constructor_type();
            let is_constructor = matches!(ty.kind, TypeExprKind::Constructor(_));
            let code = match (is_constructor, is_in_union) {
                (false, true) => 1385,
                (false, false) => 1386,
                (true, true) => 1387,
                (true, false) => 1388,
            };
            let (start, end) = (ty.span.start as usize, ty.span.end as usize);
            self.diagnostics.push(JsDocDiagnostic {
                kind: JsDocDiagnosticKind::Parse,
                start: start as u32,
                end: end as u32,
                code,
                args: Vec::new(),
            });
            return Some(ty);
        }
        None
    }

    /// `parseTypeOperatorOrHigher`
    fn parse_type_operator_or_higher(&mut self) -> TypeExpr {
        let start = self.pos();
        match self.word() {
            Some(word @ ("keyof" | "unique" | "readonly")) => {
                let operator = match word {
                    "keyof" => TypeOperator::Keyof,
                    "unique" => TypeOperator::Unique,
                    _ => TypeOperator::Readonly,
                };
                self.next_token();
                let operand = self.parse_type_operator_or_higher();
                let kind = if operator == TypeOperator::Keyof {
                    TypeExprKind::Keyof(Box::new(operand))
                } else {
                    TypeExprKind::Operator {
                        operator,
                        operand: Box::new(operand),
                    }
                };
                self.finish(start, kind)
            }
            Some("infer") => self.parse_infer_type(),
            _ => {
                let outer = std::mem::replace(&mut self.disallow_conditional, false);
                let ty = self.parse_postfix_type_or_higher();
                self.disallow_conditional = outer;
                ty
            }
        }
    }

    /// `parseInferType`
    fn parse_infer_type(&mut self) -> TypeExpr {
        let start = self.pos();
        self.next_token();
        let (name, name_start, name_end) = self.parse_identifier();
        let constraint = self.try_parse(|this| {
            if this.parse_optional_word("extends") {
                let outer = std::mem::replace(&mut this.disallow_conditional, true);
                let constraint = this.parse_type();
                this.disallow_conditional = outer;
                if this.disallow_conditional || this.tok() != Tok::Question {
                    return Some(constraint);
                }
            }
            None
        });
        let end = constraint
            .as_ref()
            .map_or(name_end, |constraint| constraint.span.end as usize);
        let parameter = TypeParameter {
            name,
            span: Span::new(name_start as u32, end as u32),
            constraint: constraint.map(Box::new),
            default: None,
            const_: false,
            in_: false,
            out: false,
        };
        self.finish(start, TypeExprKind::Infer(parameter))
    }

    /// `parsePostfixTypeOrHigher`
    fn parse_postfix_type_or_higher(&mut self) -> TypeExpr {
        let start = self.pos();
        let mut ty = self.parse_non_array_type();
        while !self.s.newline_before {
            match self.tok() {
                Tok::Excl => {
                    self.next_token();
                    ty = self.finish(
                        start,
                        TypeExprKind::JsDoc {
                            kind: JsDocTypeKind::NonNullable,
                            is_postfix: true,
                            ty: Box::new(ty),
                        },
                    );
                }
                Tok::Question => {
                    // If the next token starts a type, this is a conditional type.
                    if self.look_ahead(|this| {
                        this.next_token();
                        this.is_start_of_type(false)
                    }) {
                        return ty;
                    }
                    self.next_token();
                    ty = self.finish(
                        start,
                        TypeExprKind::JsDoc {
                            kind: JsDocTypeKind::Nullable,
                            is_postfix: true,
                            ty: Box::new(ty),
                        },
                    );
                }
                Tok::OpenBracket => {
                    self.expect(Tok::OpenBracket);
                    if self.is_start_of_type(false) {
                        let index = self.parse_type();
                        self.expect(Tok::CloseBracket);
                        ty = self.finish(
                            start,
                            TypeExprKind::IndexedAccess {
                                object: Box::new(ty),
                                index: Box::new(index),
                            },
                        );
                    } else {
                        self.expect(Tok::CloseBracket);
                        ty = self.finish(start, TypeExprKind::Array(Box::new(ty)));
                    }
                }
                _ => return ty,
            }
        }
        ty
    }

    /// `parseNonArrayType`
    fn parse_non_array_type(&mut self) -> TypeExpr {
        let start = self.pos();
        match self.tok() {
            Tok::Star => {
                // `parseJSDocAllType`
                self.next_token();
                self.keyword(start, TypeKeyword::Any)
            }
            Tok::Question => self.parse_jsdoc_unknown_or_nullable_type(),
            Tok::Excl => {
                self.next_token();
                let ty = self.parse_non_array_type();
                self.finish(
                    start,
                    TypeExprKind::JsDoc {
                        kind: JsDocTypeKind::NonNullable,
                        is_postfix: false,
                        ty: Box::new(ty),
                    },
                )
            }
            Tok::NoSubstitutionTemplate | Tok::Str | Tok::Number | Tok::BigInt => {
                self.parse_literal_type(false)
            }
            Tok::Minus => {
                if self.look_ahead(|this| {
                    this.next_token();
                    matches!(this.tok(), Tok::Number | Tok::BigInt)
                }) {
                    self.parse_literal_type(true)
                } else {
                    self.parse_type_reference()
                }
            }
            Tok::OpenBrace => {
                if self.look_ahead(Self::is_start_of_mapped_type) {
                    self.parse_mapped_type()
                } else {
                    self.parse_type_literal()
                }
            }
            Tok::OpenBracket => self.parse_tuple_type(),
            Tok::OpenParen => self.parse_parenthesized_type(),
            Tok::TemplateHead => self.parse_template_type(),
            Tok::Ident => self.parse_non_array_word_type(start),
            _ => self.parse_type_reference(),
        }
    }

    fn parse_non_array_word_type(&mut self, start: usize) -> TypeExpr {
        let Some(word) = self.word().map(str::to_owned) else {
            return self.parse_type_reference();
        };
        match word.as_str() {
            "any" | "unknown" | "string" | "number" | "bigint" | "symbol" | "boolean"
            | "undefined" | "never" | "object" => {
                // If these are followed by a dot, parse them as a dotted type reference instead.
                let keyword = self.try_parse(|this| {
                    let keyword = match word.as_str() {
                        "any" => TypeKeyword::Any,
                        "unknown" => TypeKeyword::Unknown,
                        "string" => TypeKeyword::String,
                        "number" => TypeKeyword::Number,
                        "bigint" => TypeKeyword::BigInt,
                        "symbol" => TypeKeyword::Symbol,
                        "boolean" => TypeKeyword::Boolean,
                        "undefined" => TypeKeyword::Undefined,
                        "never" => TypeKeyword::Never,
                        _ => TypeKeyword::Object,
                    };
                    this.next_token();
                    (this.tok() != Tok::Dot).then(|| this.keyword(start, keyword))
                });
                keyword.unwrap_or_else(|| self.parse_type_reference())
            }
            "true" | "false" | "null" => self.parse_literal_type(false),
            "void" => {
                self.next_token();
                self.keyword(start, TypeKeyword::Void)
            }
            "this" => {
                self.next_token();
                let this = self.keyword(start, TypeKeyword::This);
                if self.is_word("is") && !self.s.newline_before {
                    // `parseThisTypePredicate`
                    self.next_token();
                    let ty = self.parse_type();
                    return self.finish(
                        start,
                        TypeExprKind::Predicate {
                            parameter: "this".to_owned(),
                            asserts: false,
                            ty: Some(Box::new(ty)),
                        },
                    );
                }
                this
            }
            "typeof" => {
                if self.look_ahead(|this| {
                    this.next_token();
                    this.is_word("import")
                }) {
                    self.parse_import_type()
                } else {
                    self.parse_type_query()
                }
            }
            "import" => self.parse_import_type(),
            "asserts" => {
                if self.look_ahead(|this| {
                    this.next_token();
                    matches!(this.tok(), Tok::Ident) && !this.s.newline_before
                }) {
                    self.parse_asserts_type_predicate()
                } else {
                    self.parse_type_reference()
                }
            }
            _ => self.parse_type_reference(),
        }
    }

    /// `parseJSDocUnknownOrNullableType`
    fn parse_jsdoc_unknown_or_nullable_type(&mut self) -> TypeExpr {
        let start = self.pos();
        self.next_token();
        // Where the type is `?` alone: `Foo<?>`, `{ a: ? }`, `Foo(?,`, `Foo(?)`, `(?|`.
        if matches!(
            self.tok(),
            Tok::Comma | Tok::CloseBrace | Tok::CloseParen | Tok::Gt | Tok::Eq | Tok::Bar
        ) {
            return self.keyword(start, TypeKeyword::Any);
        }
        let ty = self.parse_type();
        self.finish(
            start,
            TypeExprKind::JsDoc {
                kind: JsDocTypeKind::Nullable,
                is_postfix: false,
                ty: Box::new(ty),
            },
        )
    }

    /// `parseTypeReference`
    fn parse_type_reference(&mut self) -> TypeExpr {
        let start = self.pos();
        let names = self.parse_entity_name(1110);
        let first_start = names.first().map_or(start, |name| name.1);
        let missing = names.first().is_some_and(|name| name.0.is_empty());
        let arguments = if !self.s.newline_before && self.tok() == Tok::Lt {
            self.parse_type_arguments_list()
        } else {
            Vec::new()
        };
        let name: Vec<String> = names.into_iter().map(|name| name.0).collect();
        if missing {
            // A missing name is where the type was expected, and has no extent.
            return TypeExpr {
                span: Span::new(first_start as u32, first_start as u32),
                kind: TypeExprKind::Reference { name, arguments },
            };
        }
        self.finish(first_start, TypeExprKind::Reference { name, arguments })
    }

    /// `parseTypeArgumentsOfTypeReference`
    fn parse_type_arguments_list(&mut self) -> Vec<TypeExpr> {
        self.parse_bracketed_list(Ctx::TypeArguments, Tok::Lt, Tok::Gt, |this| {
            Some(this.parse_type())
        })
    }

    /// `parseTypeArguments`: the type arguments, if the current token is `<`.
    pub fn parse_type_arguments(&mut self) -> Option<Vec<TypeExpr>> {
        (self.tok() == Tok::Lt).then(|| self.parse_type_arguments_list())
    }

    /// `parseLiteralTypeNode`
    fn parse_literal_type(&mut self, negative: bool) -> TypeExpr {
        let start = self.pos();
        if negative {
            self.next_token();
        }
        let literal = match self.tok() {
            Tok::Str | Tok::NoSubstitutionTemplate => {
                TypeLiteral::String(StringUnits::from_units(&self.s.cooked))
            }
            Tok::Number => {
                let value = parse_number(&self.s.value);
                let value = if negative { -value } else { value };
                TypeLiteral::Number(if value == 0.0 && value.is_sign_negative() {
                    "-0".to_owned()
                } else {
                    kiln_host_abi::number_to_js_string(value)
                })
            }
            Tok::BigInt => {
                let text = bigint_text(&self.s.value);
                TypeLiteral::BigInt(if negative { format!("-{text}") } else { text })
            }
            _ => match self.word() {
                Some("true") => TypeLiteral::Boolean(true),
                Some("false") => TypeLiteral::Boolean(false),
                _ => TypeLiteral::Null,
            },
        };
        self.next_token();
        self.finish(start, TypeExprKind::Literal(literal))
    }

    /// `parseTypeQuery`
    fn parse_type_query(&mut self) -> TypeExpr {
        let start = self.pos();
        self.next_token();
        let names = self.parse_entity_name(1110);
        let arguments = if !self.s.newline_before && self.tok() == Tok::Lt {
            self.parse_type_arguments_list()
        } else {
            Vec::new()
        };
        let name = names.into_iter().map(|name| name.0).collect();
        self.finish(start, TypeExprKind::Typeof { name, arguments })
    }

    /// `parseImportType`
    fn parse_import_type(&mut self) -> TypeExpr {
        let start = self.pos();
        let is_typeof = self.parse_optional_word("typeof");
        self.expect_word("import");
        self.expect(Tok::OpenParen);
        let specifier_type = self.parse_type();
        let specifier = match &specifier_type.kind {
            TypeExprKind::Literal(TypeLiteral::String(units)) => units.to_lossy_string(),
            _ => {
                // `checkImportType`: "String literal expected."
                self.diagnostics.push(JsDocDiagnostic {
                    kind: JsDocDiagnosticKind::Checker,
                    start: specifier_type.span.start,
                    end: specifier_type.span.end,
                    code: 1141,
                    args: Vec::new(),
                });
                String::new()
            }
        };
        if self.parse_optional(Tok::Comma) {
            self.expect(Tok::OpenBrace);
            if self.is_word("with") || self.is_word("assert") {
                self.next_token();
            } else {
                self.expected("with");
            }
            self.expect(Tok::Colon);
            self.expect(Tok::OpenBrace);
            self.parse_delimited_list(Ctx::ImportAttributes, true, |this| {
                // `parseImportAttribute`
                this.next_token();
                this.expect(Tok::Colon);
                this.parse_assignment_expression_skipping();
                Some(())
            });
            self.expect(Tok::CloseBrace);
            self.parse_optional(Tok::Comma);
            self.expect(Tok::CloseBrace);
        }
        self.expect(Tok::CloseParen);
        let qualifier = if self.parse_optional(Tok::Dot) {
            self.parse_entity_name(1110)
                .into_iter()
                .map(|name| name.0)
                .collect()
        } else {
            Vec::new()
        };
        let arguments = if !self.s.newline_before && self.tok() == Tok::Lt {
            self.parse_type_arguments_list()
        } else {
            Vec::new()
        };
        self.finish(
            start,
            TypeExprKind::Import {
                specifier,
                options: None,
                qualifier,
                arguments,
                is_typeof,
            },
        )
    }

    /// The value of an import attribute: a string, which is all TypeScript accepts.
    fn parse_assignment_expression_skipping(&mut self) {
        if matches!(self.tok(), Tok::Str | Tok::Number | Tok::Ident) {
            self.next_token();
        } else {
            self.error_at_token(1109, &[]);
        }
    }

    /// `parseAssertsTypePredicate`
    fn parse_asserts_type_predicate(&mut self) -> TypeExpr {
        let start = self.pos();
        self.next_token();
        let parameter = if self.is_word("this") {
            let name = "this".to_owned();
            self.next_token();
            name
        } else {
            self.parse_identifier().0
        };
        let ty = if self.parse_optional_word("is") {
            Some(Box::new(self.parse_type()))
        } else {
            None
        };
        self.finish(
            start,
            TypeExprKind::Predicate {
                parameter,
                asserts: true,
                ty,
            },
        )
    }

    /// `parseParenthesizedType`
    fn parse_parenthesized_type(&mut self) -> TypeExpr {
        let start = self.pos();
        self.expect(Tok::OpenParen);
        let ty = self.parse_type();
        self.expect(Tok::CloseParen);
        self.finish(start, TypeExprKind::Parenthesized(Box::new(ty)))
    }

    /// `parseTemplateType`
    fn parse_template_type(&mut self) -> TypeExpr {
        let start = self.pos();
        let mut quasis = vec![StringUnits::from_units(&self.s.cooked)];
        let mut types = Vec::new();
        self.next_token();
        loop {
            types.push(self.parse_type());
            if self.tok() == Tok::CloseBrace {
                self.s.rescan_template_continuation();
                quasis.push(StringUnits::from_units(&self.s.cooked));
                let middle = self.tok() == Tok::TemplateMiddle;
                self.next_token();
                if middle {
                    continue;
                }
            } else {
                self.expected("}");
                quasis.push(StringUnits::default());
            }
            break;
        }
        self.finish(start, TypeExprKind::Template { quasis, types })
    }

    /// `isStartOfMappedType`
    fn is_start_of_mapped_type(&mut self) -> bool {
        self.next_token();
        if matches!(self.tok(), Tok::Plus | Tok::Minus) {
            self.next_token();
            return self.is_word("readonly");
        }
        if self.is_word("readonly") {
            self.next_token();
        }
        if self.tok() != Tok::OpenBracket {
            return false;
        }
        self.next_token();
        if !self.is_identifier() {
            return false;
        }
        self.next_token();
        self.is_word("in")
    }

    /// `parseMappedType`
    fn parse_mapped_type(&mut self) -> TypeExpr {
        let start = self.pos();
        self.expect(Tok::OpenBrace);
        let mut readonly = TypeModifier::None;
        if matches!(self.tok(), Tok::Plus | Tok::Minus) || self.is_word("readonly") {
            readonly = match self.tok() {
                Tok::Minus => TypeModifier::Remove,
                _ => TypeModifier::Add,
            };
            let is_keyword = self.is_word("readonly");
            self.next_token();
            if !is_keyword {
                self.expect_word("readonly");
            }
        }
        self.expect(Tok::OpenBracket);
        // `parseMappedTypeParameter`
        let (name, name_start, name_end) = self.parse_identifier_name(None);
        self.expect_word("in");
        let constraint = self.parse_type();
        let name_type = if self.parse_optional_word("as") {
            Some(Box::new(self.parse_type()))
        } else {
            None
        };
        self.expect(Tok::CloseBracket);
        let mut optional = TypeModifier::None;
        if matches!(self.tok(), Tok::Question | Tok::Plus | Tok::Minus) {
            let tok = self.tok();
            optional = match tok {
                Tok::Minus => TypeModifier::Remove,
                _ => TypeModifier::Add,
            };
            self.next_token();
            if tok != Tok::Question {
                self.expect(Tok::Question);
            }
        }
        let value = if self.parse_optional(Tok::Colon) {
            self.parse_type()
        } else {
            let at = self.s.full_start;
            TypeExpr {
                span: Span::new(at as u32, at as u32),
                kind: TypeExprKind::Unsupported {
                    syntax: String::new(),
                },
            }
        };
        self.parse_semicolon();
        self.parse_list(Ctx::TypeMembers, Self::parse_type_member);
        self.expect(Tok::CloseBrace);
        self.finish(
            start,
            TypeExprKind::Mapped {
                parameter: TypeParameter {
                    name,
                    span: Span::new(name_start as u32, name_end as u32),
                    constraint: None,
                    default: None,
                    const_: false,
                    in_: false,
                    out: false,
                },
                constraint: Box::new(constraint),
                name_type,
                value: Box::new(value),
                readonly,
                optional,
            },
        )
    }

    /// `parseTupleType`
    fn parse_tuple_type(&mut self) -> TypeExpr {
        let start = self.pos();
        let elements = self.parse_bracketed_list(
            Ctx::TupleElementTypes,
            Tok::OpenBracket,
            Tok::CloseBracket,
            |this| Some(this.parse_tuple_element_name_or_type()),
        );
        self.finish(start, TypeExprKind::Tuple(elements))
    }

    /// `parseTupleElementNameOrTupleElementType`
    fn parse_tuple_element_name_or_type(&mut self) -> TypeTupleElement {
        if self.look_ahead(Self::is_tuple_element_name) {
            let rest = self.parse_optional(Tok::DotDotDot);
            let (name, _, _) = self.parse_identifier_name(None);
            let optional = self.parse_optional(Tok::Question);
            self.expect(Tok::Colon);
            let mut element = self.parse_tuple_element_type();
            element.name = Some(name);
            element.optional |= optional;
            element.rest |= rest;
            return element;
        }
        self.parse_tuple_element_type()
    }

    fn is_tuple_element_name(&mut self) -> bool {
        if self.tok() == Tok::DotDotDot {
            self.next_token();
            return self.tok() == Tok::Ident && self.is_next_token_colon_or_question_colon();
        }
        self.tok() == Tok::Ident && self.is_next_token_colon_or_question_colon()
    }

    fn is_next_token_colon_or_question_colon(&mut self) -> bool {
        self.next_token();
        if self.tok() == Tok::Colon {
            return true;
        }
        self.tok() == Tok::Question && {
            self.next_token();
            self.tok() == Tok::Colon
        }
    }

    /// `parseTupleElementType`
    fn parse_tuple_element_type(&mut self) -> TypeTupleElement {
        if self.parse_optional(Tok::DotDotDot) {
            let ty = self.parse_type();
            return TypeTupleElement {
                name: None,
                ty,
                optional: false,
                rest: true,
            };
        }
        let ty = self.parse_type();
        // `T?` is an optional element.
        if let TypeExprKind::JsDoc {
            kind: JsDocTypeKind::Nullable,
            is_postfix: true,
            ty: inner,
        } = &ty.kind
            && inner.span.start == ty.span.start
        {
            let inner = (**inner).clone();
            return TypeTupleElement {
                name: None,
                ty: inner,
                optional: true,
                rest: false,
            };
        }
        TypeTupleElement {
            name: None,
            ty,
            optional: false,
            rest: false,
        }
    }

    // ───────────────────────────── object types ─────────────────────────────

    /// `parseTypeLiteral`
    fn parse_type_literal(&mut self) -> TypeExpr {
        let start = self.pos();
        let members = if self.expect(Tok::OpenBrace) {
            let members = self.parse_list(Ctx::TypeMembers, Self::parse_type_member);
            self.expect(Tok::CloseBrace);
            members
        } else {
            Vec::new()
        };
        self.finish(start, TypeExprKind::Object(members))
    }

    /// `isTypeMemberStart`
    fn is_type_member_start(&mut self) -> bool {
        if matches!(self.tok(), Tok::OpenParen | Tok::Lt)
            || self.is_word("get")
            || self.is_word("set")
        {
            return true;
        }
        let mut is_identifier_token = false;
        // Eat up all modifiers, but hold on to the last one in case it is actually an identifier.
        while self.word().is_some_and(is_modifier) {
            is_identifier_token = true;
            self.next_token();
        }
        // Index signatures and computed property names are type members.
        if self.tok() == Tok::OpenBracket {
            return true;
        }
        // Try to get the first property-like token following all modifiers.
        if self.is_literal_property_name() {
            is_identifier_token = true;
            self.next_token();
        }
        // If we were able to get any potential identifier, check that it is the start of a member.
        if is_identifier_token {
            return matches!(
                self.tok(),
                Tok::OpenParen | Tok::Lt | Tok::Question | Tok::Colon | Tok::Comma
            ) || self.can_parse_semicolon();
        }
        false
    }

    fn is_literal_property_name(&self) -> bool {
        matches!(
            self.tok(),
            Tok::Ident | Tok::Str | Tok::Number | Tok::BigInt
        )
    }

    /// `parseTypeMemberSemicolon`
    fn parse_type_member_semicolon(&mut self) {
        // Type members may be separated by a comma.
        if self.parse_optional(Tok::Comma) {
            return;
        }
        self.parse_semicolon();
    }

    /// `parseTypeMember`
    fn parse_type_member(&mut self) -> Option<TypeMember> {
        let start = self.pos();
        if matches!(self.tok(), Tok::OpenParen | Tok::Lt) {
            return Some(self.parse_signature_member(start, false));
        }
        if self.is_word("new")
            && self.look_ahead(|this| {
                this.next_token();
                matches!(this.tok(), Tok::OpenParen | Tok::Lt)
            })
        {
            self.next_token();
            return Some(self.parse_signature_member(start, true));
        }
        let mut readonly = false;
        while self.word().is_some_and(is_modifier)
            && self.look_ahead(|this| {
                this.next_token();
                // `canFollowModifier`
                matches!(
                    this.tok(),
                    Tok::Ident
                        | Tok::Str
                        | Tok::Number
                        | Tok::BigInt
                        | Tok::OpenBracket
                        | Tok::OpenBrace
                        | Tok::Star
                        | Tok::DotDotDot
                        | Tok::PrivateIdent
                ) && !this.s.newline_before
            })
        {
            readonly |= self.is_word("readonly");
            self.next_token();
        }
        for (word, is_getter) in [("get", true), ("set", false)] {
            if self.is_word(word)
                && self.look_ahead(|this| {
                    this.next_token();
                    !this.s.newline_before
                        && (this.is_literal_property_name() || this.tok() == Tok::OpenBracket)
                })
            {
                self.next_token();
                return self.parse_accessor_member(start, is_getter);
            }
        }
        if self.look_ahead(Self::is_index_signature) {
            return self.parse_index_signature(start, readonly);
        }
        self.parse_property_or_method_signature(start, readonly)
    }

    /// `isIndexSignature`
    fn is_index_signature(&mut self) -> bool {
        self.tok() == Tok::OpenBracket && self.look_ahead_is_unambiguously_index_signature()
    }

    fn look_ahead_is_unambiguously_index_signature(&mut self) -> bool {
        // `[` identifier `:` / `,`, or `[` modifiers identifier
        self.next_token();
        if matches!(self.tok(), Tok::DotDotDot | Tok::CloseBracket) {
            return true;
        }
        if self.word().is_some_and(is_modifier) {
            self.next_token();
            if self.is_identifier() {
                return true;
            }
        } else if !self.is_identifier() {
            return false;
        } else {
            self.next_token();
        }
        // A colon signifies a well formed indexer. A comma should be a badly formed indexer
        // because comma expressions are not allowed in computed properties.
        if matches!(self.tok(), Tok::Colon | Tok::Comma) {
            return true;
        }
        if self.tok() != Tok::Question {
            return false;
        }
        self.next_token();
        matches!(self.tok(), Tok::Colon | Tok::Comma | Tok::CloseBracket)
    }

    /// `parseIndexSignatureDeclaration`
    fn parse_index_signature(&mut self, start: usize, readonly: bool) -> Option<TypeMember> {
        let (parameters, _) = self.parse_bracketed_parameters();
        let value = self.parse_optional_type_annotation();
        self.parse_type_member_semicolon();
        let parameter = parameters.into_iter().next()?;
        let key_type = parameter.ty?;
        let value_type = value?;
        Some(TypeMember {
            name: parameter.name.clone(),
            key: None,
            span: Span::new(start as u32, self.end() as u32),
            optional: false,
            readonly,
            this_parameter: None,
            kind: TypeMemberKind::Index {
                parameter: parameter.name.unwrap_or_default(),
                key_type,
                value_type,
            },
        })
    }

    /// `parseBracketedList(Parameters, parseParameter, [, ])`
    fn parse_bracketed_parameters(
        &mut self,
    ) -> (Vec<TypeSignatureParameter>, Option<TypeSignatureParameter>) {
        let mut parameters = Vec::new();
        if self.expect(Tok::OpenBracket) {
            parameters = self.parse_delimited_list(Ctx::Parameters, false, Self::parse_parameter);
            self.expect(Tok::CloseBracket);
        }
        (parameters, None)
    }

    /// `parseAccessorDeclaration`
    fn parse_accessor_member(&mut self, start: usize, is_getter: bool) -> Option<TypeMember> {
        let (name, key, _) = self.parse_property_name();
        let type_parameters = self.parse_type_parameters();
        let (parameters, this_parameter) = self.parse_parameters();
        let return_type = self.parse_return_type(Tok::Colon, false);
        self.parse_type_member_semicolon();
        let span = Span::new(start as u32, self.end() as u32);
        let rest = parameters.iter().any(|p| p.rest);
        let signature = TypeSignature {
            span,
            type_parameters,
            this_parameter: this_parameter.map(Box::new),
            parameters,
            rest,
            return_type: return_type.clone().map(Box::new),
        };
        Some(TypeMember {
            name,
            key,
            span,
            optional: false,
            readonly: false,
            this_parameter: signature.this_parameter.clone(),
            kind: if is_getter {
                TypeMemberKind::Getter(return_type)
            } else {
                TypeMemberKind::Setter(signature)
            },
        })
    }

    /// `parseSignatureMember`
    fn parse_signature_member(&mut self, start: usize, is_construct: bool) -> TypeMember {
        let type_parameters = self.parse_type_parameters();
        let (parameters, this_parameter) = self.parse_parameters();
        let return_type = self.parse_return_type(Tok::Colon, true);
        self.parse_type_member_semicolon();
        let span = Span::new(start as u32, self.end() as u32);
        let rest = parameters.iter().any(|p| p.rest);
        let signature = TypeSignature {
            span,
            type_parameters,
            this_parameter: this_parameter.map(Box::new),
            parameters,
            rest,
            return_type: return_type.map(Box::new),
        };
        TypeMember {
            name: None,
            key: None,
            span,
            optional: false,
            readonly: false,
            this_parameter: signature.this_parameter.clone(),
            kind: if is_construct {
                TypeMemberKind::Construct(signature)
            } else {
                TypeMemberKind::Call(signature)
            },
        }
    }

    /// `parsePropertyName`: the name, the key, and where the name starts. A computed name has none.
    fn parse_property_name(&mut self) -> (Option<String>, Option<kiln_hir::ObjectKey>, usize) {
        let at = self.pos();
        match self.tok() {
            Tok::Ident => {
                let name = self.s.value.clone();
                self.next_token();
                (
                    Some(name.clone()),
                    Some(kiln_hir::ObjectKey::Static(StringUnits::from(name))),
                    at,
                )
            }
            Tok::Str => {
                let units = StringUnits::from_units(&self.s.cooked);
                let name = units.to_lossy_string();
                self.next_token();
                (Some(name), Some(kiln_hir::ObjectKey::Static(units)), at)
            }
            Tok::Number => {
                let name = kiln_host_abi::number_to_js_string(parse_number(&self.s.value));
                self.next_token();
                (
                    Some(name.clone()),
                    Some(kiln_hir::ObjectKey::Static(StringUnits::from(name))),
                    at,
                )
            }
            Tok::BigInt => {
                let name = bigint_text(&self.s.value);
                self.next_token();
                (
                    Some(name.clone()),
                    Some(kiln_hir::ObjectKey::Static(StringUnits::from(name))),
                    at,
                )
            }
            Tok::PrivateIdent => {
                let name = self.s.value.clone();
                self.next_token();
                (
                    Some(name.clone()),
                    Some(kiln_hir::ObjectKey::Private(name)),
                    at,
                )
            }
            Tok::OpenBracket => {
                // A computed property name: `[expression]`. Only its extent is read.
                self.next_token();
                let mut depth = 1usize;
                while depth > 0 && self.tok() != Tok::Eof {
                    match self.tok() {
                        Tok::OpenBracket => depth += 1,
                        Tok::CloseBracket => depth -= 1,
                        _ => {}
                    }
                    self.next_token();
                }
                (None, None, at)
            }
            _ => {
                // `parsePropertyNameWorker`: an identifier name is expected.
                let (name, _, _) = self.parse_identifier_name(Some(1003));
                (Some(name), None, at)
            }
        }
    }

    /// `parsePropertyOrMethodSignature`
    fn parse_property_or_method_signature(
        &mut self,
        start: usize,
        readonly: bool,
    ) -> Option<TypeMember> {
        let (name, key, _) = self.parse_property_name();
        let optional = self.parse_optional(Tok::Question);
        if matches!(self.tok(), Tok::OpenParen | Tok::Lt) {
            let type_parameters = self.parse_type_parameters();
            let (parameters, this_parameter) = self.parse_parameters();
            let return_type = self.parse_return_type(Tok::Colon, true);
            self.parse_type_member_semicolon();
            let span = Span::new(start as u32, self.end() as u32);
            let rest = parameters.iter().any(|p| p.rest);
            let signature = TypeSignature {
                span,
                type_parameters,
                this_parameter: this_parameter.map(Box::new),
                parameters,
                rest,
                return_type: return_type.map(Box::new),
            };
            return Some(TypeMember {
                name,
                key,
                span,
                optional,
                readonly: false,
                this_parameter: signature.this_parameter.clone(),
                kind: TypeMemberKind::Method(signature),
            });
        }
        let ty = self.parse_optional_type_annotation();
        if self.tok() == Tok::Eq {
            // `parseInitializer`: "Property signatures cannot have initializers" is the checker's.
            self.next_token();
            self.skip_initializer();
        }
        self.parse_type_member_semicolon();
        let span = Span::new(start as u32, self.end() as u32);
        let ty = ty.unwrap_or_else(|| TypeExpr {
            span: Span::new(span.end, span.end),
            kind: TypeExprKind::Unsupported {
                syntax: String::new(),
            },
        });
        Some(TypeMember {
            name,
            key,
            span,
            optional,
            readonly,
            this_parameter: None,
            kind: TypeMemberKind::Property(ty),
        })
    }

    /// The expression of an initializer, of which only the extent matters.
    fn skip_initializer(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.tok() {
                Tok::Eof => return,
                Tok::OpenParen | Tok::OpenBracket | Tok::OpenBrace => depth += 1,
                Tok::CloseParen | Tok::CloseBracket | Tok::CloseBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Tok::Comma | Tok::Semi if depth == 0 => return,
                _ => {}
            }
            self.next_token();
            if depth == 0 && self.s.newline_before {
                return;
            }
        }
    }

    /// `parseTypeAnnotation`: `: T`, if there is a colon.
    fn parse_optional_type_annotation(&mut self) -> Option<TypeExpr> {
        self.parse_optional(Tok::Colon).then(|| self.parse_type())
    }

    // ───────────────────────────── signatures ─────────────────────────────

    /// `parseTypeParameters`
    pub fn parse_type_parameters(&mut self) -> Vec<TypeParameter> {
        if self.tok() == Tok::Lt {
            return self.parse_bracketed_list(Ctx::TypeParameters, Tok::Lt, Tok::Gt, |this| {
                Some(this.parse_type_parameter())
            });
        }
        Vec::new()
    }

    /// `parseTypeParameter`
    fn parse_type_parameter(&mut self) -> TypeParameter {
        let start = self.pos();
        let (mut const_, mut in_, mut out) = (false, false, false);
        while let Some(word) = self
            .word()
            .filter(|word| matches!(*word, "const" | "in" | "out"))
            .map(str::to_owned)
        {
            let is_modifier_of_name = self.look_ahead(|this| {
                this.next_token();
                this.is_identifier() && !this.s.newline_before
            });
            if !is_modifier_of_name {
                break;
            }
            match word.as_str() {
                "const" => const_ = true,
                "in" => in_ = true,
                _ => out = true,
            }
            self.next_token();
        }
        let (name, _, _) = self.parse_identifier();
        let mut constraint = None;
        if self.parse_optional_word("extends") {
            constraint = Some(Box::new(self.parse_type()));
        }
        let default = if self.parse_optional(Tok::Eq) {
            Some(Box::new(self.parse_type()))
        } else {
            None
        };
        TypeParameter {
            name,
            span: Span::new(start as u32, self.end() as u32),
            constraint,
            default,
            const_,
            in_,
            out,
        }
    }

    /// `parseParameters`: the parameters, and the `this` parameter that is among them.
    fn parse_parameters(
        &mut self,
    ) -> (Vec<TypeSignatureParameter>, Option<TypeSignatureParameter>) {
        if !self.expect(Tok::OpenParen) {
            return (Vec::new(), None);
        }
        let mut parameters =
            self.parse_delimited_list(Ctx::Parameters, false, Self::parse_parameter);
        self.expect(Tok::CloseParen);
        let this = (parameters.first().and_then(|p| p.name.as_deref()) == Some("this"))
            .then(|| parameters.remove(0));
        (parameters, this)
    }

    /// `parseParameter`
    fn parse_parameter(&mut self) -> Option<TypeSignatureParameter> {
        let start = self.pos();
        // Modifiers (`public x`) are an error in a type, but part of the parameter.
        while self.word().is_some_and(is_modifier)
            && self.look_ahead(|this| {
                this.next_token();
                this.is_identifier() || matches!(this.tok(), Tok::OpenBracket | Tok::OpenBrace)
            })
        {
            self.next_token();
        }
        if self.is_word("this") {
            let name_start = self.pos();
            self.next_token();
            let name_end = self.end();
            let ty = self.parse_optional_type_annotation();
            return Some(TypeSignatureParameter {
                name: Some("this".to_owned()),
                pattern: Some(Box::new(kiln_hir::Pattern::Identifier("this".to_owned()))),
                pattern_span: Span::new(name_start as u32, name_end as u32),
                span: Span::new(start as u32, self.end() as u32),
                ty,
                optional: false,
                rest: false,
                has_default: false,
            });
        }
        let rest = self.parse_optional(Tok::DotDotDot);
        let (name, pattern, pattern_span) =
            if matches!(self.tok(), Tok::OpenBracket | Tok::OpenBrace) {
                let at = self.pos();
                self.skip_binding_pattern();
                (None, None, Span::new(at as u32, self.end() as u32))
            } else {
                let (name, name_start, name_end) = self.parse_identifier();
                (
                    Some(name.clone()),
                    Some(Box::new(kiln_hir::Pattern::Identifier(name))),
                    Span::new(name_start as u32, name_end as u32),
                )
            };
        let optional = self.parse_optional(Tok::Question);
        let ty = self.parse_optional_type_annotation();
        let mut has_default = false;
        if self.tok() == Tok::Eq {
            self.next_token();
            self.skip_initializer();
            has_default = true;
        }
        Some(TypeSignatureParameter {
            name,
            pattern,
            pattern_span,
            span: Span::new(start as u32, self.end() as u32),
            ty,
            optional,
            rest,
            has_default,
        })
    }
}

fn token_text(tok: Tok) -> &'static str {
    match tok {
        Tok::OpenBrace => "{",
        Tok::CloseBrace => "}",
        Tok::OpenParen => "(",
        Tok::CloseParen => ")",
        Tok::OpenBracket => "[",
        Tok::CloseBracket => "]",
        Tok::Lt => "<",
        Tok::Gt => ">",
        Tok::Comma => ",",
        Tok::Colon => ":",
        Tok::Semi => ";",
        Tok::Dot => ".",
        Tok::DotDotDot => "...",
        Tok::Question => "?",
        Tok::Excl => "!",
        Tok::Star => "*",
        Tok::Eq => "=",
        Tok::Arrow => "=>",
        Tok::Bar => "|",
        Tok::Amp => "&",
        Tok::At => "@",
        Tok::Minus => "-",
        Tok::Plus => "+",
        Tok::Slash => "/",
        Tok::Backtick => "`",
        _ => "",
    }
}

fn parse_number(text: &str) -> f64 {
    let lower = text.to_ascii_lowercase();
    for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if let Some(digits) = lower.strip_prefix(prefix) {
            return u128::from_str_radix(digits, radix).map_or(f64::NAN, |value| value as f64);
        }
    }
    lower.parse().unwrap_or(f64::NAN)
}

fn bigint_text(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if let Some(digits) = lower.strip_prefix(prefix)
            && let Some(value) = num_bigint::BigUint::parse_bytes(digits.as_bytes(), radix)
        {
            return value.to_string();
        }
    }
    let digits = lower.trim_start_matches('0');
    if digits.is_empty() {
        "0".to_owned()
    } else {
        digits.to_owned()
    }
}

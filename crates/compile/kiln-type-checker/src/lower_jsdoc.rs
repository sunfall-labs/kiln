//! The tags of the JSDoc comments of a JavaScript file as ordinary nodes: TypeScript's
//! `reparser.go`, as Bun's `reparse.rs` ports it.
//!
//! The lowering pass calls [`Lower::with_jsdoc`] for every node TypeScript's parser calls
//! `withJSDoc` for, once the node is lowered. Hosted tags modify that node: they add annotations,
//! type parameters, modifiers, casts. Unhosted tags produce separate statements, which stay in
//! [`JsDocState::reparsed`] until they are inserted into their statement list.
use crate::{
    engine::{
        atom::Atom,
        hir::{self as b, parentheses_around},
    },
    lower::Lower,
};
use kiln_hir as k;
use kiln_hir::jsdoc::*;
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

/// The node a JSDoc comment is attached to, to the extent that hosted tags distinguish node kinds.
pub(super) enum Host {
    /// Hosted tags on it are ignored.
    Other,
    VariableStatement(b::Span<b::VarDeclId>),
    VariableDeclaration(b::VarDeclId),
    /// `export default e`, `export = e`
    ExportAssignment(b::StmtId),
    ExpressionStatement(b::StmtId),
    ReturnStatement(b::StmtId),
    /// The expression inside the parentheses.
    Parenthesized(b::ExprId),
    Parameter(b::ParamId),
    /// A function declaration, a function expression or an arrow function.
    Function(b::FnId),
    Class(b::ClassId),
    /// A member of a class, before it is added to the file.
    ClassMember(b::Member),
    /// A property of an object literal, before it is added to the file, and the type of its
    /// `@type` tag.
    Property(b::Prop, b::TypeNodeId),
}

/// What the reparser has built and not yet put in a place.
pub(super) struct JsDocState {
    /// Statements synthesized from tags, until they are inserted in their statement list.
    pub reparsed: Vec<b::StmtId>,
    pub reparsed_members: Vec<b::Member>,
    /// Modifiers that tags add to the member being reparsed.
    member_modifiers: Vec<(b::Flags, u32)>,
    /// Object literals the lowering is inside of.
    pub object_literals_around: u32,
    /// Functions that a `@type` tag gives a whole signature (`FullSignature`).
    full_signatures: FxHashSet<u32>,
    documented_functions: FxHashSet<u32>,
    /// Comments attached to a node: only their syntax errors are reported.
    is_attached: Vec<bool>,
    /// `checkGrammarModuleElementContext`: the statement list is that of a file or a module body.
    pub is_in_appropriate_context: bool,
    /// The statements `with_jsdoc` made, which are positioned at their tags.
    pub reparsed_statements: FxHashSet<u32>,
    hosts: Vec<b::JsDocHost>,
}

impl JsDocState {
    pub fn new(docs: usize) -> Self {
        JsDocState {
            reparsed: Vec::new(),
            reparsed_members: Vec::new(),
            member_modifiers: Vec::new(),
            object_literals_around: 0,
            full_signatures: FxHashSet::default(),
            documented_functions: FxHashSet::default(),
            is_attached: vec![false; docs],
            is_in_appropriate_context: true,
            reparsed_statements: FxHashSet::default(),
            hosts: Vec::new(),
        }
    }
}

/// `GetJSDocCommentRanges`: the JSDoc comments before a token.
struct Attached {
    /// Those that have tags, as indices into `JsDocSyntax::docs`.
    docs: SmallVec<[u32; 2]>,
    /// The last of `docs` is the last of all the JSDoc comments.
    last_has_tags: bool,
    /// From the start of the first of all the JSDoc comments to the end of the last. `None`: there
    /// is none.
    range: Option<b::TextRange>,
    /// `jsdocScannerInfoHasSeeOrLink`
    has_see_or_link: bool,
}

/// The modifier flags in `flags`, which an overload signature shares with the implementation.
fn modifiers_of(flags: b::Flags) -> b::Flags {
    flags.difference(b::Flags::GENERATOR | b::Flags::OPTIONAL | b::Flags::MISSING_BODY)
}

/// `IsValidIdentifier`
fn is_valid_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let is_start = |c: char| c == '$' || c == '_' || c.is_alphabetic();
    chars.next().is_some_and(is_start) && chars.all(|c| is_start(c) || c.is_alphanumeric())
}

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

impl<'a, 's> Lower<'a, 's> {
    // ───────────────────────────── attaching comments to nodes ─────────────────────────────

    pub(super) fn has_jsdoc(&self) -> bool {
        !self.module.type_syntax.jsdoc.docs.is_empty()
    }

    /// The trivia before the token at `token`: the comments directly before it, and the end of
    /// the token before them (the full start of the node).
    fn comments_before(&self, token: u32) -> (u32, std::ops::Range<usize>) {
        let comments = &self.module.type_syntax.jsdoc.comments;
        let text = self.module.source.as_bytes();
        let mut cursor = (token as usize).min(text.len());
        let mut first = comments.partition_point(|c| c.start < token);
        loop {
            let mut blank = cursor;
            while blank > 0 && is_blank(text[blank - 1]) {
                blank -= 1;
            }
            // The comment that ends at `blank`.
            let Some(before) = first.checked_sub(1) else {
                cursor = blank;
                break;
            };
            if comments[before].end as usize == blank {
                cursor = comments[before].start as usize;
                first = before;
            } else {
                cursor = blank;
                break;
            }
        }
        let end = comments.partition_point(|c| c.start < token);
        (cursor as u32, first..end)
    }

    /// The JSDoc comments of the node whose first token is at `token`. `with_trailing`: comments
    /// on the same line as the previous token are included, as they are for a parameter, a
    /// variable declaration, a parenthesized expression and a function expression.
    fn jsdoc_before(&self, token: u32, with_trailing: bool) -> Attached {
        let syntax = &self.module.type_syntax.jsdoc;
        let text = self.module.source.as_bytes();
        let (full_start, range) = self.comments_before(token);
        let mut attached = Attached {
            docs: SmallVec::new(),
            last_has_tags: false,
            range: None,
            has_see_or_link: false,
        };
        // `GetLeadingCommentRanges`: the comments after the first line break, or after the start
        // of the file.
        let mut is_collecting = with_trailing || full_start == 0;
        let mut previous_end = full_start as usize;
        for comment in &syntax.comments[range] {
            attached.has_see_or_link |= comment.see_or_link;
            let has_line_break_before = text
                .get(previous_end..comment.start as usize)
                .is_some_and(|between| between.iter().any(|&c| c == b'\n' || c == b'\r'));
            previous_end = comment.end as usize;
            is_collecting |= has_line_break_before;
            if !is_collecting || !comment.jsdoc_like {
                // A line comment ends with its line.
                is_collecting |= comment.single_line;
                continue;
            }
            attached.range = Some(b::TextRange {
                pos: attached.range.map_or(comment.start, |range| range.pos),
                end: comment.end,
            });
            attached.last_has_tags = match syntax.at(comment.start) {
                Some(index) if !syntax.docs[index].tags.is_empty() => {
                    attached.docs.push(index as u32);
                    true
                }
                Some(index) => {
                    attached.docs.push(index as u32);
                    false
                }
                None => false,
            };
        }
        attached
    }

    /// `withJSDoc` for the node `host` whose first token is at `token`.
    pub(super) fn with_jsdoc(&mut self, token: u32, with_trailing: bool, host: &mut Host) {
        if !self.has_jsdoc() {
            return;
        }
        let attached = self.jsdoc_before(token, with_trailing);
        if let Some(comments) = attached.range {
            let syntax = &self.module.type_syntax.jsdoc;
            let docs = attached.docs.iter();
            let mut tags = docs.flat_map(|&doc| &syntax.docs[doc as usize].tags);
            let first_satisfies_tag =
                tags.find(|tag| matches!(tag.kind, JsDocTagKind::Satisfies(_)));
            let first_satisfies_tag = first_satisfies_tag.map_or(u32::MAX, |tag| tag.name_pos);
            self.jsdoc_state.hosts.push(b::JsDocHost {
                token,
                comments,
                first_satisfies_tag,
            });
        }
        if !attached.docs.is_empty() {
            self.reparse_tags(host, &attached);
        }
    }

    /// `withJSDoc` for the statement `statement`, which was lowered to `id`.
    pub(super) fn statement_jsdoc(&mut self, statement: &k::Statement, id: b::StmtId) {
        use k::StatementKind as S;
        let mut host = match (&statement.kind, self.file[id].kind) {
            (S::Variable { .. }, b::StmtKind::Var(decls)) => Host::VariableStatement(decls),
            // `parseExpressionOrLabeledStatement`: a statement that starts with a parenthesis
            // leaves the comment to the parenthesized expression.
            (S::Expression(_), b::StmtKind::Expr(_))
                if self
                    .module
                    .source
                    .as_bytes()
                    .get(statement.span.start as usize)
                    == Some(&b'(') =>
            {
                return;
            }
            (S::Expression(_), b::StmtKind::Expr(_)) => Host::ExpressionStatement(id),
            (S::Return(_), b::StmtKind::Return(_)) => Host::ReturnStatement(id),
            (S::Function(_), b::StmtKind::Fn(func)) => Host::Function(func),
            (S::Class(_), b::StmtKind::Class(class)) => Host::Class(class),
            (S::ExportDefault(_), _) => Host::ExportAssignment(id),
            _ => Host::Other,
        };
        self.with_jsdoc(statement.span.start, false, &mut host);
    }

    /// `parseClassElement`: a `;` is a member, and has its comments.
    pub(super) fn semicolon_elements_jsdoc(&mut self, class: k::Span, members: &[b::Member]) {
        let syntax = &self.module.type_syntax.jsdoc;
        let text = self.module.source.as_bytes();
        let first = syntax.comments.partition_point(|c| c.start < class.start);
        let mut semicolons = Vec::new();
        for (index, comment) in syntax.comments.iter().enumerate().skip(first) {
            if comment.start >= class.end {
                break;
            }
            let mut at = comment.end as usize;
            let mut next = index + 1;
            loop {
                while text.get(at).is_some_and(|&c| is_blank(c)) {
                    at += 1;
                }
                match syntax.comments.get(next) {
                    Some(following) if following.start as usize == at => {
                        at = following.end as usize;
                        next += 1;
                    }
                    _ => break,
                }
            }
            let inside_member = members
                .iter()
                .any(|m| m.start as usize <= at && at < m.loc.end as usize);
            if text.get(at) == Some(&b';') && !inside_member && at < class.end as usize {
                semicolons.push(at as u32);
            }
        }
        semicolons.dedup();
        for semicolon in semicolons {
            self.with_jsdoc(semicolon, false, &mut Host::Other);
        }
    }

    /// The same for a member of a class whose first token is at `start`.
    pub(super) fn member_jsdoc(&mut self, member: b::Member, start: u32) -> b::Member {
        if !self.has_jsdoc() {
            return member;
        }
        self.jsdoc_state.member_modifiers.clear();
        let mut host = Host::ClassMember(member);
        self.with_jsdoc(start, false, &mut host);
        let Host::ClassMember(mut member) = host else {
            return member;
        };
        if self.jsdoc_state.member_modifiers.is_empty() {
            return member;
        }
        // Modifiers synthesized from tags come after those in the source.
        let mut all: Vec<b::Modifier> = self.file.modifier_list(member.modifiers).to_vec();
        for (flag, pos) in std::mem::take(&mut self.jsdoc_state.member_modifiers) {
            all.push(b::Modifier {
                kind: b::ModifierKind::Keyword(flag),
                pos,
            });
        }
        member.modifiers = self.file.add_modifiers(&all);
        member
    }

    /// Called once everything is lowered: stores the information collected about the comments in
    /// the file.
    pub(super) fn finish_jsdoc(&mut self) {
        // `parseJSDocComment`: "move jsdoc diagnostics to jsdocDiagnostics -- for JS files only"
        if !self.file.is_js {
            return;
        }
        let syntax = &self.module.type_syntax.jsdoc;
        let file = &mut self.file;
        file.jsdoc_asterisks.clone_from(&syntax.asterisks);
        for (doc, &is_attached) in syntax.docs.iter().zip(&self.jsdoc_state.is_attached) {
            file.jsdoc_comments.push((doc.start, doc.end));
            if is_attached {
                let parse_errors = doc
                    .diagnostics
                    .iter()
                    .filter(|d| d.kind == JsDocDiagnosticKind::Parse);
                file.diagnostics
                    .extend(parse_errors.map(|d| to_diagnostic(d, b::DiagnosticKind::JsDoc)));
            }
        }
        // Only `findOriginatingJSDocSatisfiesTag` asks for them.
        let mut hosts = std::mem::take(&mut self.jsdoc_state.hosts);
        if hosts
            .iter()
            .any(|host| host.first_satisfies_tag != u32::MAX)
        {
            hosts.sort_by_key(|host| host.token);
            hosts.dedup_by_key(|host| host.token);
            file.jsdoc_hosts = hosts.into();
        }
        file.jsdoc_types.sort_unstable_by_key(|t| t.0);
        file.jsdoc_modifiers.sort_unstable_by_key(|m| m.0);
        file.jsdoc_member_comments.sort_unstable_by_key(|m| m.0);
    }

    // ───────────────────────────── helpers ─────────────────────────────

    fn name_atom(&self, name: &JsDocName) -> Atom {
        self.atom(&name.text)
    }

    /// The outermost parentheses around `e`: their start and end.
    fn paren_of(&self, e: b::ExprId) -> Option<(u32, u32)> {
        // In creation order, which is id order.
        let outermost = parentheses_around(&self.file, e).last()?;
        Some((outermost.1, outermost.2))
    }

    /// `IsPrivateIdentifier` for the name of a property access.
    fn is_private_name(&self, name: Atom) -> bool {
        self.atoms.bytes(name).first() == Some(&b'#')
    }

    /// `IsEntityNameExpressionEx`, in JavaScript.
    fn is_entity_name_expression(&self, e: b::ExprId) -> bool {
        if self.paren_of(e).is_some() {
            return false;
        }
        let file = &self.file;
        match file[e].kind {
            b::ExprKind::Ident(_) | b::ExprKind::This => true,
            b::ExprKind::Dot { obj, name, .. } => {
                !self.is_private_name(name) && self.is_entity_name_expression(obj)
            }
            b::ExprKind::Index { obj, index, .. } => {
                self.paren_of(index).is_none()
                    && match file[index].kind {
                        b::ExprKind::String(_) | b::ExprKind::Number(_) => true,
                        b::ExprKind::Template { exprs, .. } => exprs.is_empty(),
                        _ => false,
                    }
                    && self.is_entity_name_expression(obj)
            }
            _ => false,
        }
    }

    /// `GetAssignmentDeclarationKind(e) != JSDeclarationKindNone` for a binary expression in
    /// JavaScript.
    fn is_assignment_declaration(&self, e: b::ExprId) -> bool {
        let file = &self.file;
        let b::ExprKind::Assign {
            op: None, target, ..
        } = file[e].kind
        else {
            return false;
        };
        if self.paren_of(e).is_some() || self.paren_of(target).is_some() {
            return false;
        }
        let is_this = |obj: b::ExprId| {
            matches!(file[obj].kind, b::ExprKind::This) && self.paren_of(obj).is_none()
        };
        match file[target].kind {
            b::ExprKind::Dot { obj, name, .. } => {
                is_this(obj) || !self.is_private_name(name) && self.is_entity_name_expression(obj)
            }
            b::ExprKind::Index { obj, .. } => is_this(obj) || self.is_entity_name_expression(obj),
            _ => false,
        }
    }

    /// The expression of an expression statement, a `return` or an `export default`.
    fn statement_expression(&self, stmt: b::StmtId) -> b::ExprId {
        match self.file[stmt].kind {
            b::StmtKind::Expr(e)
            | b::StmtKind::Return(e)
            | b::StmtKind::ExportDefault(e)
            | b::StmtKind::ExportAssign(e) => e,
            _ => b::ExprId::NONE,
        }
    }

    fn set_statement_expression(&mut self, stmt: b::StmtId, e: b::ExprId) {
        let kind = &mut self.file[stmt].kind;
        match kind {
            b::StmtKind::Expr(_) => *kind = b::StmtKind::Expr(e),
            b::StmtKind::Return(_) => *kind = b::StmtKind::Return(e),
            b::StmtKind::ExportDefault(_) => *kind = b::StmtKind::ExportDefault(e),
            b::StmtKind::ExportAssign(_) => *kind = b::StmtKind::ExportAssign(e),
            _ => {}
        }
    }

    /// The function that `e` is after skipping any `satisfies` (`skipSatisfiesExpressions`).
    /// `NONE` if it is not a function.
    fn function_expression(&self, mut e: b::ExprId) -> b::FnId {
        let file = &self.file;
        while e.is_some() && self.paren_of(e).is_none() {
            match file[e].kind {
                b::ExprKind::Satisfies { expr, .. } => e = expr,
                b::ExprKind::Fn(func) => return func,
                _ => break,
            }
        }
        b::FnId::NONE
    }

    /// `getFunctionLikeHost`
    fn function_like_host(&self, host: &Host) -> b::FnId {
        let file = &self.file;
        match *host {
            Host::Function(func) => func,
            Host::ClassMember(member) => match member.kind {
                b::MemberKind::Property => self.function_expression(member.init),
                b::MemberKind::Method
                | b::MemberKind::Getter
                | b::MemberKind::Setter
                | b::MemberKind::Constructor => member.func,
                _ => b::FnId::NONE,
            },
            Host::Property(prop, _) => match prop.kind {
                b::PropKind::Init
                | b::PropKind::Method
                | b::PropKind::Getter
                | b::PropKind::Setter => self.function_expression(prop.value),
                b::PropKind::Shorthand | b::PropKind::Spread => b::FnId::NONE,
            },
            Host::VariableStatement(decls) if !decls.is_empty() => {
                self.function_expression(file[decls.at(0)].init)
            }
            Host::ExportAssignment(stmt) | Host::ReturnStatement(stmt) => {
                self.function_expression(self.statement_expression(stmt))
            }
            Host::ExpressionStatement(stmt) => {
                // `GetRightMostAssignedExpression`
                let mut e = self.statement_expression(stmt);
                while e.is_some()
                    && self.paren_of(e).is_none()
                    && let b::ExprKind::Assign { value, .. } = file[e].kind
                {
                    e = value;
                }
                self.function_expression(e)
            }
            _ => b::FnId::NONE,
        }
    }

    /// `FullSignature != nil`
    fn has_full_signature(&self, func: b::FnId) -> bool {
        self.jsdoc_state.full_signatures.contains(&func.0)
    }

    /// `checkNonIdentifierName`. The reparser's own errors count as parser errors, not as errors
    /// of the comment.
    fn check_non_identifier_name(&mut self, name: &JsDocName) {
        if !is_valid_identifier(&name.text) {
            // A missing name is reported at the character before it.
            let start = if name.is_missing() {
                name.start.saturating_sub(1)
            } else {
                name.start
            };
            self.file
                .error(b::DiagnosticKind::Parse, start, name.end, 1003);
        }
    }

    // ───────────────────────────── types ─────────────────────────────

    /// Reports the checker errors in the comment syntax from `start` to `end`, which is being
    /// reparsed.
    fn note_checker_errors(&mut self, start: u32, end: u32) {
        let docs = &self.module.type_syntax.jsdoc.docs;
        let after = docs.partition_point(|doc| doc.start <= start);
        if let Some(doc) = after.checked_sub(1).map(|index| &docs[index]) {
            let checker_errors = doc.diagnostics.iter().filter(|d| {
                d.kind != JsDocDiagnosticKind::Parse && (start..end).contains(&d.start)
            });
            self.file
                .diagnostics
                .extend(checker_errors.map(|d| to_diagnostic(d, b::DiagnosticKind::JsDoc)));
        }
    }

    /// `addDeepCloneReparse` for the type of a type expression.
    fn reparse_type(&mut self, expr: &JsDocTypeExpr) -> b::TypeNodeId {
        // `checkSourceElementWorker` has no case for a `JSDocVariadicType` or a
        // `JSDocOptionalType`.
        if expr.is_variadic || expr.is_optional {
            return self.reparse_unchecked_type(expr);
        }
        self.note_checker_errors(expr.pos, expr.end);
        self.clone_type_expression(expr)
    }

    /// The same for a type that `checkSourceFile` never reaches: the checker only calls
    /// `getTypeFromTypeNode` for it.
    fn reparse_unchecked_type(&mut self, expr: &JsDocTypeExpr) -> b::TypeNodeId {
        self.clone_type_expression(expr)
    }

    fn clone_type_expression(&mut self, expr: &JsDocTypeExpr) -> b::TypeNodeId {
        let mut ty = match &expr.ty {
            Some(ty) => self.ty(ty),
            None => b::TypeNodeId::NONE,
        };
        if ty.is_none() {
            ty = self.file.ty(
                b::TypeNodeKind::Keyword(b::Keyword::Any),
                expr.pos,
                expr.pos,
            );
        }
        let end = self.file[ty].end;
        // `parseJSDocType`
        if expr.is_variadic {
            ty = self.file.ty(
                b::TypeNodeKind::JSDoc {
                    ty,
                    kind: b::JSDocTypeKind::Variadic,
                    is_postfix: false,
                },
                expr.pos,
                end,
            );
        }
        if expr.is_optional {
            ty = self.file.ty(
                b::TypeNodeKind::JSDoc {
                    ty,
                    kind: b::JSDocTypeKind::Optional,
                    is_postfix: true,
                },
                expr.pos,
                end,
            );
        }
        ty
    }

    /// `reparseJSDocTypeLiteral`
    fn reparse_tag_type(&mut self, ty: &JsDocTagType) -> b::TypeNodeId {
        let (properties, is_array, pos) = match ty {
            JsDocTagType::None => return b::TypeNodeId::NONE,
            JsDocTagType::Expr(expr) => return self.reparse_type(expr),
            JsDocTagType::Literal {
                properties,
                is_array,
                pos,
            } => (properties, *is_array, *pos),
        };
        let mut members = Vec::with_capacity(properties.len());
        let mut comments: Vec<(usize, String)> = Vec::new();
        for tag in properties {
            let (JsDocTagKind::Property(property) | JsDocTagKind::Param(property)) = &tag.kind
            else {
                continue;
            };
            let Some(name) = property.name.last() else {
                continue;
            };
            let mut flags = b::Flags::REPARSED;
            // A name that is not an identifier is still a name, as a string.
            if !is_valid_identifier(&name.text) {
                flags |= b::Flags::STRING_NAME | b::Flags::LITERAL_NAME;
            }
            if Self::is_optional(property) {
                flags |= b::Flags::OPTIONAL;
            }
            if !property.comment.is_empty() {
                comments.push((members.len(), property.comment.clone()));
            }
            let ty = self.reparse_tag_type(&property.ty);
            members.push(b::Member {
                kind: b::MemberKind::Property,
                key: b::PropKey::Name(self.name_atom(name)),
                flags,
                ty,
                init: b::ExprId::NONE,
                func: b::FnId::NONE,
                name_pos: name.start,
                start: tag.pos,
                loc: b::TextRange {
                    pos: tag.pos,
                    end: tag.end,
                },
                modifiers: b::Span::EMPTY,
            });
        }
        let end = members.last().map_or(pos, |last| last.loc.end);
        let members = self.file.add_members(&members);
        for (index, comment) in comments {
            self.file
                .jsdoc_member_comments
                .push((members.at(index), comment.into_bytes().into()));
        }
        let literal = self.file.ty(b::TypeNodeKind::Object(members), pos, end);
        if is_array {
            self.file.ty(b::TypeNodeKind::Array(literal), pos, end)
        } else {
            literal
        }
    }

    /// `makeQuestionIfOptional`
    fn is_optional(property: &JsDocProperty) -> bool {
        property.is_bracketed
            || matches!(&property.ty, JsDocTagType::Expr(expr) if expr.is_optional)
    }

    /// `gatherTypeParameters`
    fn gather_type_parameters(
        &mut self,
        doc: &JsDocComment,
        typedef_or_callback: bool,
    ) -> b::Span<b::TypeParamId> {
        // In a comment with a `@typedef` or a `@callback` the `@template` tags apply to the type
        // being defined.
        if !typedef_or_callback
            && doc.tags.iter().any(|tag| {
                matches!(
                    tag.kind,
                    JsDocTagKind::Typedef(_) | JsDocTagKind::Callback(_)
                )
            })
        {
            return b::Span::EMPTY;
        }
        let mut params = Vec::new();
        for tag in &doc.tags {
            let JsDocTagKind::Template(template) = &tag.kind else {
                continue;
            };
            for (index, param) in template.params.iter().enumerate() {
                let constraint = match &template.constraint {
                    Some(constraint) if index == 0 => {
                        self.check_non_identifier_name(&param.name);
                        self.reparse_type(constraint)
                    }
                    _ => b::TypeNodeId::NONE,
                };
                let default = match &param.default {
                    Some(default) => self.reparse_type(default),
                    None => b::TypeNodeId::NONE,
                };
                let mut flags = b::Flags::REPARSED;
                let mut modifiers = Vec::new();
                for &(modifier, pos) in &param.modifiers {
                    let flag = match modifier {
                        JsDocTypeParameterModifier::Const => b::Flags::CONST,
                        JsDocTypeParameterModifier::In => b::Flags::IN,
                        JsDocTypeParameterModifier::Out => b::Flags::OUT,
                    };
                    flags |= flag;
                    modifiers.push(b::Modifier {
                        kind: b::ModifierKind::Keyword(flag),
                        pos,
                    });
                }
                let modifiers = self.file.add_modifiers(&modifiers);
                params.push(b::TypeParam {
                    name: self.name_atom(&param.name),
                    pos: param.name.start,
                    start: param.pos,
                    end: param.end,
                    constraint,
                    default,
                    flags,
                    modifiers,
                });
            }
        }
        self.file.add_type_params(&params)
    }

    /// `makeNewCast`
    fn make_cast(&mut self, ty: &JsDocTypeExpr, e: b::ExprId, is_assertion: bool) -> b::ExprId {
        let b::Expr { pos, end, .. } = self.file[e];
        let (pos, end) = self.paren_of(e).unwrap_or((pos, end));
        // `isConstTypeReference`
        let is_const = is_assertion
            && !ty.is_variadic
            && !ty.is_optional
            && ty.ty.as_ref().is_some_and(|written| {
                // A `ParenthesizedType` has no node: it starts before the node of its type.
                written.span.start == ty.pos
                    && matches!(&written.kind, k::TypeExprKind::Reference { name, arguments }
                        if arguments.is_empty() && name.len() == 1 && name[0] == "const")
            });
        let kind = if is_const {
            b::ExprKind::AsConst(e)
        } else if is_assertion {
            b::ExprKind::As {
                expr: e,
                ty: self.reparse_type(ty),
            }
        } else {
            b::ExprKind::Satisfies {
                expr: e,
                ty: self.reparse_type(ty),
            }
        };
        self.file.expr(kind, pos, end)
    }

    // ───────────────────────────── tags ─────────────────────────────

    /// `reparseTags`, which `withJSDoc` calls in JavaScript only.
    fn reparse_tags(&mut self, host: &mut Host, attached: &Attached) {
        let syntax = &self.module.type_syntax.jsdoc;
        let is_js = self.file.is_js;
        // `EagerJSDoc`: in TypeScript `withJSDoc` parses the comments only if one has a link.
        if let Host::Class(class) = *host
            && (is_js || attached.has_see_or_link)
        {
            self.check_grammar_augments_tags(class, attached);
        }
        for (i, &index) in attached.docs.iter().enumerate() {
            self.jsdoc_state.is_attached[index as usize] = true;
            let doc = &syntax.docs[index as usize];
            let is_last = attached.last_has_tags && i + 1 == attached.docs.len();
            let reparsed: &[JsDocTag] = if is_js { &doc.tags } else { &[] };
            for tag in reparsed {
                self.reparse_unhosted(tag, host, doc);
                if is_last {
                    self.reparse_hosted(tag, host, doc);
                }
            }
            if is_last {
                self.note_unmatched_parameters(host, doc);
            }
        }
    }

    /// The type alias for a `@typedef` or a `@callback`, wrapped in the namespaces of its
    /// qualified name (`wrapInJSDocNamespace`).
    fn reparse_alias(
        &mut self,
        name: &JsDocDeclaredName,
        ty: b::TypeNodeId,
        tag: &JsDocTag,
        host: &Host,
        doc: &JsDocComment,
    ) {
        let type_params = self.gather_type_parameters(doc, true);
        let mut flags = b::Flags::REPARSED;
        if !name.namespaces.is_empty() {
            flags |= b::Flags::EXPORT;
        }
        let alias = b::Alias {
            name: self.name_atom(&name.name),
            name_pos: name.name.start,
            flags,
            type_params,
            ty,
            stmt: b::StmtId::NONE,
        };
        let alias = self.file.add_alias(alias);
        let mut statement = self.file.stmt(b::StmtKind::TypeAlias(alias), tag.pos);
        self.file[statement].loc = b::TextRange {
            pos: tag.pos,
            end: tag.end,
        };
        self.jsdoc_state.reparsed_statements.insert(statement.0);
        // The binder exports the outermost one, only from a module
        // (`IsImplicitlyExportedJSDocDeclaration`).
        for (depth, namespace) in name.namespaces.iter().enumerate().rev() {
            let module = b::Module {
                name: b::ModuleName::Ident(self.name_atom(namespace)),
                name_pos: namespace.start,
                flags: if depth > 0 {
                    b::Flags::REPARSED | b::Flags::EXPORT
                } else if matches!(host, Host::ClassMember(_)) {
                    b::Flags::REPARSED | b::Flags::CLASS_ELEMENT
                } else {
                    b::Flags::REPARSED
                },
                body: self.file.list(&[statement]),
                has_body: true,
                specifies_module: false,
                stmt: b::StmtId::NONE,
            };
            let module = self.file.add_module(module);
            statement = self.file.stmt(b::StmtKind::Module(module), namespace.start);
            // `parseJSDocTypeNameWithNamespace`: the rest of the name is its body.
            self.file[statement].loc = b::TextRange {
                pos: namespace.start,
                end: name.name.end,
            };
            self.jsdoc_state.reparsed_statements.insert(statement.0);
        }
        // `parseListIndex` passes only type aliases and imports on to a list of statements, so
        // the namespace is a member of the class (`checkGrammarModuleElementContext`).
        if let (Some(outermost), Host::ClassMember(_)) = (name.namespaces.first(), host) {
            self.file
                .error(b::DiagnosticKind::Grammar, outermost.start, 0, 1235);
        }
        self.jsdoc_state.reparsed.push(statement);
    }

    /// `reparseUnhosted`
    fn reparse_unhosted(&mut self, tag: &JsDocTag, host: &Host, doc: &JsDocComment) {
        match &tag.kind {
            JsDocTagKind::Typedef(typedef) => {
                if matches!(typedef.ty, JsDocTagType::None) {
                    return;
                }
                self.check_non_identifier_name(&typedef.name.name);
                let ty = self.reparse_tag_type(&typedef.ty);
                self.reparse_alias(&typedef.name, ty, tag, host, doc);
            }
            JsDocTagKind::Callback(callback) => {
                let signature = self.reparse_signature(&callback.signature, None, doc, tag);
                let kind = b::TypeNodeKind::Fn(signature);
                let ty = self.file.ty(kind, callback.signature.pos, tag.end);
                self.reparse_alias(&callback.name, ty, tag, host, doc);
            }
            JsDocTagKind::Import(import) => {
                if !import.has_clause {
                    return;
                }
                // `checkImportDeclaration` returns after `checkGrammarModuleElementContext`.
                if self.jsdoc_state.is_in_appropriate_context {
                    self.note_checker_errors(tag.pos, import.end);
                }
                let mode = match import.mode {
                    Some(JsDocResolutionMode::Import) => b::ResolutionMode::Import,
                    Some(JsDocResolutionMode::Require) => b::ResolutionMode::Require,
                    _ => b::ResolutionMode::None,
                };
                let spec = match &import.specifier {
                    Some((text, pos)) => {
                        let spec = self.atom(text);
                        self.file.specifier_uses.push(b::SpecifierUse {
                            spec,
                            pos: *pos,
                            kind: b::SpecifierKind::Import,
                            mode,
                        });
                        spec
                    }
                    None => Atom::NONE,
                };
                let named: Vec<b::ImportSpec> = import
                    .named
                    .iter()
                    .map(|specifier| b::ImportSpec {
                        start: specifier.start,
                        imported: self.atom(&specifier.imported),
                        local: self.atom(&specifier.local),
                        pos: specifier.local_pos,
                        type_only: specifier.is_type_only,
                        imported_pos: specifier.imported_pos,
                        end: specifier.local_pos + specifier.local.len() as u32,
                        import: b::ImportId(self.file.imports.len() as u32),
                    })
                    .collect();
                let declaration = b::Import {
                    spec,
                    default: import
                        .default
                        .as_ref()
                        .map_or(Atom::NONE, |name| self.name_atom(name)),
                    default_pos: import.default.as_ref().map_or(0, |name| name.start),
                    namespace: import
                        .namespace
                        .as_ref()
                        .map_or(Atom::NONE, |name| self.name_atom(name)),
                    namespace_pos: import.namespace.as_ref().map_or(0, |name| name.start),
                    clause_start: import.clause_start,
                    // Only read by the diagnostics for `import defer`.
                    clause_end: import.clause_end,
                    namespace_start: import.namespace_start,
                    named: self.file.add_import_specs(&named),
                    has_named_imports: import.has_named_imports,
                    type_only: true,
                    is_deferred: false,
                    mode,
                    stmt: b::StmtId::NONE,
                };
                let declaration = self.file.add_import(declaration);
                let statement = self.file.stmt(b::StmtKind::Import(declaration), tag.pos);
                self.file[statement].loc = b::TextRange {
                    pos: tag.pos,
                    end: tag.end,
                };
                self.jsdoc_state.reparsed_statements.insert(statement.0);
                self.jsdoc_state.reparsed.push(statement);
            }
            JsDocTagKind::Overload(signature) => {
                // Only for function, method and constructor declarations outside of object
                // literals.
                if self.jsdoc_state.object_literals_around != 0 {
                    return;
                }
                match *host {
                    Host::Function(func) if self.file[func].kind == b::FnKind::Decl => {
                        let signature = self.reparse_signature(signature, Some(func), doc, tag);
                        let statement = self.file.stmt(b::StmtKind::Fn(signature), tag.name_pos);
                        // `tag.TagName()`
                        self.file[statement].loc = b::TextRange {
                            pos: tag.name_pos,
                            end: tag.name_pos + "overload".len() as u32,
                        };
                        self.jsdoc_state.reparsed_statements.insert(statement.0);
                        self.jsdoc_state.reparsed.push(statement);
                    }
                    Host::ClassMember(member)
                        if matches!(
                            member.kind,
                            b::MemberKind::Method | b::MemberKind::Constructor
                        ) && !matches!(member.key, b::PropKey::Computed(_)) =>
                    {
                        let func = self.reparse_signature(signature, Some(member.func), doc, tag);
                        self.jsdoc_state.reparsed_members.push(b::Member {
                            flags: modifiers_of(member.flags) | b::Flags::REPARSED,
                            ty: b::TypeNodeId::NONE,
                            init: b::ExprId::NONE,
                            func,
                            name_pos: tag.name_pos,
                            start: tag.name_pos,
                            // `tag.TagName()`
                            loc: b::TextRange {
                                pos: tag.name_pos,
                                end: tag.name_pos + "overload".len() as u32,
                            },
                            ..member
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// `reparseJSDocSignature`. `like`: the function an `@overload` tag is on. Without it the
    /// signature is the function type of a `@callback`.
    fn reparse_signature(
        &mut self,
        signature: &JsDocSignature,
        like: Option<b::FnId>,
        doc: &JsDocComment,
        tag: &JsDocTag,
    ) -> b::FnId {
        // `checkNonIdentifierName(fun.Name())`: of the identifiers that the main parser accepts
        // as the name of a function or a method, only a missing one is not valid.
        if let Some(like) = like {
            let like = self.file[like];
            if matches!(like.kind, b::FnKind::Decl | b::FnKind::Method)
                && like.name == self.atom("")
                && !like
                    .flags
                    .intersects(b::Flags::COMPUTED_NAME | b::Flags::LITERAL_NAME)
            {
                self.check_non_identifier_name(&JsDocName::missing(like.name_pos));
            }
        }
        let type_params = match like {
            Some(_) => self.gather_type_parameters(doc, false),
            None => b::Span::EMPTY,
        };
        let mut this_param = b::ParamId::NONE;
        let mut params = Vec::with_capacity(signature.params.len());
        for (index, param) in signature.params.iter().enumerate() {
            let property = match &param.kind {
                JsDocTagKind::This(ty, text) => {
                    if this_param.is_none() {
                        // `thisIdent.Loc = thisTag.Loc`
                        let name = self.atom(text);
                        let this = b::Param {
                            pat: self.file.pat(b::PatKind::Ident(name), param.pos, param.end),
                            ty: self.reparse_type(ty),
                            default: b::ExprId::NONE,
                            flags: b::Flags::REPARSED,
                            pos: param.pos,
                            loc: b::TextRange {
                                pos: param.pos,
                                end: param.end,
                            },
                        };
                        this_param = self.file.add_param(this);
                    }
                    continue;
                }
                JsDocTagKind::Param(property) | JsDocTagKind::Property(property) => property,
                _ => continue,
            };
            // `@param x.y` describes a property of another parameter.
            let [name] = &property.name[..] else {
                continue;
            };
            let mut flags = b::Flags::REPARSED;
            let ty = match &property.ty {
                // The type after the dots is used unchanged as the type of the parameter.
                JsDocTagType::Expr(expr) if expr.is_variadic && !expr.is_optional => {
                    flags |= b::Flags::REST;
                    let unwrapped = JsDocTypeExpr {
                        is_variadic: false,
                        ..expr.clone()
                    };
                    self.reparse_type(&unwrapped)
                }
                other => self.reparse_tag_type(other),
            };
            if Self::is_optional(property) {
                flags |= b::Flags::OPTIONAL;
            }
            let name_atom = if is_valid_identifier(&name.text) {
                self.name_atom(name)
            } else if name.text.is_empty() {
                self.atom(&format!("_{index}"))
            } else {
                // Characters that are invalid in an identifier are replaced with `_`.
                let is_start = |c: char| c.is_alphabetic() || c == '_' || c == '$';
                let spelled: String = name
                    .text
                    .chars()
                    .enumerate()
                    .map(|(i, c)| {
                        if is_start(c) || i > 0 && c.is_ascii_digit() {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                self.atom(&spelled)
            };
            let pat = self
                .file
                .pat(b::PatKind::Ident(name_atom), name.start, name.end);
            params.push(b::Param {
                pat,
                ty,
                default: b::ExprId::NONE,
                flags,
                pos: param.pos,
                loc: b::TextRange {
                    pos: param.pos,
                    end: param.end,
                },
            });
        }
        let pos = match like {
            Some(_) => tag.name_pos,
            None => signature.pos,
        };
        let ret = match (&signature.ret, like) {
            (Some(ret), _) => self.reparse_type(ret),
            (None, Some(_)) => b::TypeNodeId::NONE,
            (None, None) => self
                .file
                .ty(b::TypeNodeKind::Keyword(b::Keyword::Any), pos, pos),
        };
        let (kind, flags, name) = match like {
            Some(like) => {
                let like = &self.file[like];
                (like.kind, modifiers_of(like.flags), like.name)
            }
            None => (b::FnKind::FunctionType, b::Flags::empty(), Atom::NONE),
        };
        let mut params = self.file.add_params(&params);
        // `GetThisParameter`. A parameter created from an `@this` tag precedes every `@param`
        // tag.
        if this_param.is_none() {
            (this_param, params) = self.file.split_this_parameter(params);
        }
        self.file.add_fn(b::Func {
            kind,
            flags: flags | b::Flags::REPARSED,
            name,
            name_pos: pos,
            type_params,
            params,
            this_param,
            ret,
            body: b::FnBody::None,
            anchor: pos,
            start: pos,
        })
    }

    /// `reparseHosted`
    fn reparse_hosted(&mut self, tag: &JsDocTag, host: &mut Host, doc: &JsDocComment) {
        match &tag.kind {
            JsDocTagKind::Type(ty) => {
                let mut type_tags = doc
                    .tags
                    .iter()
                    .filter(|t| matches!(t.kind, JsDocTagKind::Type(_)));
                let is_last = type_tags
                    .next_back()
                    .is_some_and(|last| last.pos == tag.pos);
                self.reparse_type_tag(ty, host, is_last);
            }
            JsDocTagKind::Satisfies(ty) => self.reparse_satisfies_tag(ty, host),
            JsDocTagKind::Template(_) => {
                let func = self.function_like_host(host);
                if func.is_some() {
                    if self.file[func].type_params.is_empty() && !self.has_full_signature(func) {
                        let type_params = self.gather_type_parameters(doc, false);
                        self.file[func].type_params = type_params;
                        // `checkGrammarConstructorTypeParameters`: the list starts with its first
                        // tag.
                        if self.file[func].kind == b::FnKind::Constructor && !type_params.is_empty()
                        {
                            self.file
                                .error(b::DiagnosticKind::Grammar, tag.pos, tag.end, 1092);
                        }
                    }
                } else if let Host::Class(class) = *host
                    && self.file[class].type_params.is_empty()
                {
                    let type_params = self.gather_type_parameters(doc, false);
                    self.file[class].type_params = type_params;
                }
            }
            JsDocTagKind::Param(property) => {
                let func = self.function_like_host(host);
                if func.is_none() || self.has_full_signature(func) {
                    return;
                }
                let Some(param) = self.find_matching_parameter(func, tag, property, doc) else {
                    return;
                };
                if self.file[param].ty.is_none() && !matches!(property.ty, JsDocTagType::None) {
                    let ty = self.reparse_tag_type(&property.ty);
                    self.file[param].ty = ty;
                }
                if !self.file[param].flags.contains(b::Flags::OPTIONAL)
                    && Self::is_optional(property)
                {
                    self.file[param].flags |= b::Flags::OPTIONAL | b::Flags::REPARSED;
                    // `checkGrammarParameterList`, `checkGrammarAccessor`: the position of the `?`
                    // is that of the tag.
                    let b::Func {
                        kind,
                        type_params,
                        params,
                        ret,
                        ..
                    } = self.file[func];
                    if self.file[param].flags.contains(b::Flags::REST) {
                        self.file
                            .error(b::DiagnosticKind::Grammar, tag.pos, tag.end, 1047);
                    } else if kind == b::FnKind::Setter
                        && type_params.is_empty()
                        && params.len() == 1
                        && ret.is_none()
                    {
                        self.file
                            .error(b::DiagnosticKind::Grammar, tag.pos, tag.end, 1051);
                    }
                }
            }
            JsDocTagKind::This(ty, _) => {
                let func = self.function_like_host(host);
                if func.is_some() && self.file[func].this_param.is_none() {
                    // `finishReparsedNode(thisParam, tag.TagName())`
                    let pat = self
                        .file
                        .pat(b::PatKind::Missing, tag.name_pos, tag.name_pos);
                    let this = b::Param {
                        pat,
                        ty: self.reparse_type(ty),
                        default: b::ExprId::NONE,
                        flags: b::Flags::REPARSED,
                        pos: tag.name_pos,
                        loc: b::TextRange {
                            pos: tag.name_pos,
                            end: tag.name_pos + "this".len() as u32,
                        },
                    };
                    let this = self.file.add_param(this);
                    self.file[func].this_param = this;
                    // `checkParameter`: the position of the parameter is that of the tag's name.
                    let code = match self.file[func].kind {
                        b::FnKind::Arrow => Some(2730),
                        b::FnKind::Constructor => Some(2681),
                        b::FnKind::Getter | b::FnKind::Setter => Some(2784),
                        _ => None,
                    };
                    if let Some(code) = code {
                        self.file
                            .error(b::DiagnosticKind::Checker, tag.name_pos, 0, code);
                    }
                }
            }
            JsDocTagKind::Return(Some(ty)) => {
                let func = self.function_like_host(host);
                if func.is_some() && !self.has_full_signature(func) && self.file[func].ret.is_none()
                {
                    let ty = self.reparse_type(ty);
                    self.file[func].ret = ty;
                }
            }
            JsDocTagKind::Modifier(modifier) => self.reparse_modifier_tag(*modifier, tag, host),
            JsDocTagKind::Implements(class_name) => {
                // A missing name is never resolved.
                if let Host::Class(class) = *host
                    && class_name
                        .name
                        .first()
                        .is_some_and(|first| !first.is_missing())
                {
                    let implemented = self.reparse_class_name(class_name);
                    let mut all: Vec<b::TypeNodeId> =
                        self.file.ids(self.file[class].implements).collect();
                    all.push(implemented);
                    let all = self.file.list(&all);
                    self.file[class].implements = all;
                }
            }
            JsDocTagKind::Augments(class_name, _) => {
                if let Host::Class(class) = *host {
                    self.reparse_augments_tag(class_name, class);
                }
            }
            _ => {}
        }
    }

    /// `A.B<T>` as a type.
    fn reparse_class_name(&mut self, class_name: &JsDocClassName) -> b::TypeNodeId {
        let name: SmallVec<[(Atom, u32); 4]> = class_name
            .name
            .iter()
            .map(|name| (self.name_atom(name), name.start))
            .collect();
        let args = self.reparse_type_arguments(class_name);
        let name = self.file.entity_name(name.into_iter());
        let pos = class_name.name.first().map_or(0, |first| first.start);
        let kind = b::TypeNodeKind::Ref { name, args };
        self.file.ty(kind, pos, class_name.end)
    }

    fn reparse_type_arguments(&mut self, class_name: &JsDocClassName) -> b::IdList<b::TypeNodeId> {
        let Some(type_args) = &class_name.type_args else {
            return b::IdList::EMPTY;
        };
        let start = class_name.name.first().map_or(0, |first| first.start);
        self.note_checker_errors(start, class_name.end);
        self.ty_list(type_args)
    }

    /// `checkGrammarClassDeclarationHeritageClauses`: the last name of an `@augments` or
    /// `@extends` tag, in any comment of the class, is the last name of the `extends` clause. It
    /// returns at the first error. The checker decides between this error and the other errors of
    /// that function (`Checker::check_grammar_augments_tags`).
    fn check_grammar_augments_tags(&mut self, class: b::ClassId, attached: &Attached) {
        let extends = self.file[class].extends;
        if extends.is_none() || self.paren_of(extends).is_some() {
            return;
        }
        // `getIdentifierFromEntityNameExpression`
        let (b::ExprKind::Ident(target) | b::ExprKind::Dot { name: target, .. }) =
            self.file[extends].kind
        else {
            return;
        };
        let syntax = &self.module.type_syntax.jsdoc;
        let docs = attached.docs.iter();
        for tag in docs.flat_map(|&index| &syntax.docs[index as usize].tags) {
            if let JsDocTagKind::Augments(class_name, tag_name) = &tag.kind
                && let Some(source) = class_name.name.last()
                && target != self.name_atom(source)
            {
                let target = self.atoms.bytes(target).to_vec();
                let names: [&[u8]; 3] = [tag_name.as_bytes(), source.text.as_bytes(), &target];
                let at = (source.start, source.end);
                let diagnostic = b::Diagnostic::new(b::DiagnosticKind::Grammar, at, 8023, &names);
                self.file.diagnostics.push(diagnostic);
                self.file
                    .unmatched_augments_tags
                    .push((class, source.start));
                return;
            }
        }
    }

    /// `@augments`, `@extends`: the type arguments are added to the `extends` clause, if it names
    /// the same class.
    fn reparse_augments_tag(&mut self, class_name: &JsDocClassName, class: b::ClassId) {
        let b::Class {
            extends,
            extends_args,
            ..
        } = self.file[class];
        if extends.is_none() {
            return;
        }
        // From the last name to the first.
        let mut written = Vec::new();
        let mut at = extends;
        let is_entity_name = loop {
            if self.paren_of(at).is_some() {
                break false;
            }
            match self.file[at].kind {
                b::ExprKind::Ident(name) => {
                    written.push(name);
                    break true;
                }
                b::ExprKind::Dot { obj, name, .. } => {
                    written.push(name);
                    at = obj;
                }
                _ => break false,
            }
        };
        // `HasSamePropertyAccessName`
        let is_same = is_entity_name
            && written.len() == class_name.name.len()
            && written
                .iter()
                .rev()
                .zip(&class_name.name)
                .all(|(&a, b)| a == self.name_atom(b));
        if is_same && extends_args.is_empty() && class_name.type_args.is_some() {
            let args = self.reparse_type_arguments(class_name);
            self.file[class].extends_args = args;
        }
    }

    /// `@type`
    fn reparse_type_tag(&mut self, ty: &JsDocTypeExpr, host: &mut Host, is_last_type_tag: bool) {
        match host {
            Host::VariableStatement(decls) => {
                let first = decls.iter().find(|&decl| self.file[decl].ty.is_none());
                if let Some(decl) = first {
                    let ty = self.reparse_type(ty);
                    self.file[decl].ty = ty;
                    return;
                }
            }
            Host::VariableDeclaration(decl) => {
                if self.file[*decl].ty.is_none() {
                    let ty = self.reparse_type(ty);
                    self.file[*decl].ty = ty;
                    return;
                }
            }
            Host::ExportAssignment(stmt) => {
                let owner = b::JsDocTypeOwner::Export(*stmt);
                if self.file.jsdoc_types.last().is_none_or(|t| t.0 != owner) {
                    let ty = self.reparse_unchecked_type(ty);
                    self.file.jsdoc_types.push((owner, ty));
                    return;
                }
            }
            Host::ClassMember(member) if member.kind == b::MemberKind::Property => {
                if member.ty.is_none() {
                    member.ty = self.reparse_type(ty);
                    return;
                }
            }
            Host::ClassMember(b::Member {
                kind: b::MemberKind::Getter,
                func,
                ..
            }) => {
                if self.file[*func].ret.is_none() {
                    let ty = self.reparse_type(ty);
                    self.file[*func].ret = ty;
                    return;
                }
            }
            Host::Property(prop, prop_ty) => match prop.kind {
                b::PropKind::Init | b::PropKind::Shorthand => {
                    if prop_ty.is_none() {
                        *prop_ty = self.reparse_unchecked_type(ty);
                        return;
                    }
                }
                b::PropKind::Getter => {
                    let func = self.function_expression(prop.value);
                    if func.is_some() && self.file[func].ret.is_none() {
                        let ty = self.reparse_type(ty);
                        self.file[func].ret = ty;
                        return;
                    }
                }
                _ => {}
            },
            Host::Parameter(param) => {
                if self.file[*param].ty.is_none() {
                    let ty = self.reparse_type(ty);
                    self.file[*param].ty = ty;
                    return;
                }
            }
            Host::ExpressionStatement(stmt) => {
                let e = self.statement_expression(*stmt);
                if e.is_some() && self.is_assignment_declaration(e) {
                    // `SetType` without a test: the last tag replaces the others.
                    if is_last_type_tag {
                        let ty = self.reparse_unchecked_type(ty);
                        let owner = b::JsDocTypeOwner::Assign(e);
                        self.file.jsdoc_types.push((owner, ty));
                    }
                    return;
                }
            }
            Host::ReturnStatement(stmt) => {
                let e = self.statement_expression(*stmt);
                if e.is_some() {
                    let cast = self.make_cast(ty, e, true);
                    self.set_statement_expression(*stmt, cast);
                    return;
                }
            }
            Host::Parenthesized(e) => {
                *e = self.make_cast(ty, *e, true);
                return;
            }
            _ => {}
        }
        let func = self.function_like_host(host);
        if func.is_none() || self.has_full_signature(func) {
            return;
        }
        let function = self.file[func];
        let b::Func {
            type_params,
            params,
            ret,
            ..
        } = function;
        let has_no_typed_params = function.this_ty(&self.file).is_none()
            && params.iter().all(|param| self.file[param].ty.is_none());
        if type_params.is_empty() && ret.is_none() && has_no_typed_params {
            let ty = self.reparse_unchecked_type(ty);
            self.file
                .jsdoc_types
                .push((b::JsDocTypeOwner::Fn(func), ty));
            self.jsdoc_state.full_signatures.insert(func.0);
        }
    }

    /// `@satisfies`
    fn reparse_satisfies_tag(&mut self, ty: &JsDocTypeExpr, host: &mut Host) {
        match host {
            Host::VariableStatement(decls) => {
                let first = decls.iter().find(|&decl| self.file[decl].init.is_some());
                if let Some(decl) = first {
                    let init = self.file[decl].init;
                    let cast = self.make_cast(ty, init, false);
                    self.file[decl].init = cast;
                }
            }
            Host::VariableDeclaration(decl) => {
                let init = self.file[*decl].init;
                if init.is_some() {
                    let cast = self.make_cast(ty, init, false);
                    self.file[*decl].init = cast;
                }
            }
            Host::ClassMember(member) if member.kind == b::MemberKind::Property => {
                if member.init.is_some() {
                    member.init = self.make_cast(ty, member.init, false);
                }
            }
            Host::Property(prop, _) if prop.kind == b::PropKind::Init => {
                if prop.value.is_some() {
                    prop.value = self.make_cast(ty, prop.value, false);
                }
            }
            Host::ReturnStatement(stmt) | Host::ExportAssignment(stmt) => {
                let e = self.statement_expression(*stmt);
                if e.is_some() {
                    let cast = self.make_cast(ty, e, false);
                    self.set_statement_expression(*stmt, cast);
                }
            }
            Host::Parenthesized(e) => *e = self.make_cast(ty, *e, false),
            Host::ExpressionStatement(stmt) => {
                let e = self.statement_expression(*stmt);
                if e.is_some()
                    && self.is_assignment_declaration(e)
                    && let b::ExprKind::Assign { op, target, value } = self.file[e].kind
                {
                    let value = self.make_cast(ty, value, false);
                    self.file[e].kind = b::ExprKind::Assign { op, target, value };
                }
            }
            _ => {}
        }
    }

    /// `@public`, `@private`, `@protected`, `@readonly`, `@override`
    fn reparse_modifier_tag(&mut self, modifier: JsDocModifier, tag: &JsDocTag, host: &mut Host) {
        let modifier = match modifier {
            JsDocModifier::Public => b::Flags::PUBLIC,
            JsDocModifier::Private => b::Flags::PRIVATE,
            JsDocModifier::Protected => b::Flags::PROTECTED,
            JsDocModifier::Readonly => b::Flags::READONLY,
            JsDocModifier::Override => b::Flags::OVERRIDE,
        };
        match host {
            Host::ExpressionStatement(stmt) => {
                let e = self.statement_expression(*stmt);
                if e.is_some()
                    && self.paren_of(e).is_none()
                    && matches!(self.file[e].kind, b::ExprKind::Assign { .. })
                {
                    match self.file.jsdoc_modifiers.last_mut() {
                        Some(last) if last.0 == e => last.1 |= modifier,
                        _ => self.file.jsdoc_modifiers.push((e, modifier)),
                    }
                }
            }
            Host::ClassMember(member) => {
                let is_modified = match member.kind {
                    // They are not modifiers in an object literal, nor in anything nested in one.
                    b::MemberKind::Method | b::MemberKind::Getter | b::MemberKind::Setter => {
                        self.jsdoc_state.object_literals_around == 0
                    }
                    b::MemberKind::Property | b::MemberKind::Constructor => true,
                    _ => false,
                };
                if is_modified {
                    member.flags |= modifier;
                    if member.func.is_some() {
                        self.file[member.func].flags |= modifier;
                    }
                    self.jsdoc_state
                        .member_modifiers
                        .push((modifier | b::Flags::REPARSED, tag.pos));
                }
            }
            _ => {}
        }
    }

    /// `findMatchingParameter`
    fn find_matching_parameter(
        &self,
        func: b::FnId,
        tag: &JsDocTag,
        property: &JsDocProperty,
        doc: &JsDocComment,
    ) -> Option<b::ParamId> {
        let tag_index = doc
            .tags
            .iter()
            .filter(|other| matches!(other.kind, JsDocTagKind::Param(_)))
            .position(|other| std::ptr::eq(other, tag));
        let tag_name = match &property.name[..] {
            [name] => Some(name),
            _ => None,
        };
        let file = &self.file;
        file[func]
            .params
            .iter()
            .enumerate()
            .find(|&(index, param)| match file[file[param].pat].kind {
                b::PatKind::Ident(name) => tag_name.is_some_and(|tag_name| {
                    name == self.name_atom(tag_name)
                        || Some(index) == tag_index && tag_name.is_missing()
                }),
                _ => Some(index) == tag_index,
            })
            .map(|(_, param)| param)
    }

    /// `checkUnmatchedJSDocParameters`, to the extent that syntax alone determines it.
    /// `getAllJSDocTags`: only the tags of the nearest comment are used, which is the first one to
    /// reach this function.
    fn note_unmatched_parameters(&mut self, host: &Host, doc: &JsDocComment) {
        let func = match *host {
            // `GetNextJSDocCommentLocation` does not go through an assignment.
            Host::ExpressionStatement(stmt) => {
                self.function_expression(self.statement_expression(stmt))
            }
            Host::VariableDeclaration(decl) => self.function_expression(self.file[decl].init),
            _ => self.function_like_host(host),
        };
        if func.is_none() || !self.jsdoc_state.documented_functions.insert(func.0) {
            return;
        }
        let tags: Vec<&JsDocProperty> = doc
            .tags
            .iter()
            .filter_map(|tag| match &tag.kind {
                JsDocTagKind::Param(property)
                    if !matches!(&property.name[..], [name] if name.is_missing()) =>
                {
                    Some(property)
                }
                _ => None,
            })
            .collect();
        let Some(&last) = tags.last() else {
            return;
        };
        self.file.functions_with_param_tags.push(func);
        // `isJs`
        if !self.file.is_js {
            return;
        }
        let mut names = Vec::new();
        let mut is_pattern = Vec::new();
        for param in self.file[func].params.iter() {
            match self.file[self.file[param].pat].kind {
                b::PatKind::Ident(name) => {
                    names.push(name);
                    is_pattern.push(false);
                }
                b::PatKind::Object(_) | b::PatKind::Array(_) => is_pattern.push(true),
                b::PatKind::Missing => is_pattern.push(false),
            }
        }
        let is_matched = |this: &Self, index: usize, property: &JsDocProperty| {
            is_pattern.get(index) == Some(&true)
                || matches!(&property.name[..], [name] if names.contains(&this.name_atom(name)))
        };
        let mut errors: Vec<(&[JsDocName], u32)> = Vec::new();
        // If the function refers to `arguments`, the last tag may describe it.
        if let [_] = last.name[..]
            && !is_matched(self, tags.len() - 1, last)
            && !matches!(last.ty, JsDocTagType::None)
            && !self.is_array_type(&last.ty)
        {
            errors.push((&last.name, 8029));
        }
        for (index, &property) in tags.iter().enumerate() {
            if is_matched(self, index, property) {
                continue;
            }
            match property.name[..] {
                [_] if !property.is_name_first => errors.push((&property.name, 8024)),
                [_] | [] => {}
                [..] => errors.push((&property.name, 8032)),
            }
        }
        for (name, code) in errors {
            // `entityNameToString` of the name and, for a qualified name, of the part before its
            // last dot.
            let parts: Vec<&str> = name.iter().map(|part| part.text.as_str()).collect();
            let whole = parts.join(".");
            let left = parts[..parts.len() - 1].join(".");
            let args: Vec<&[u8]> = if code == 8032 {
                vec![whole.as_bytes(), left.as_bytes()]
            } else {
                vec![whole.as_bytes()]
            };
            let at = (name[0].start, name[name.len() - 1].end);
            let diagnostic = b::Diagnostic::new(b::DiagnosticKind::Checker, at, code, &args);
            self.file.jsdoc_param_errors.push((func, diagnostic));
        }
    }

    /// `isArrayType`, decided from the syntax of the type.
    fn is_array_type(&self, ty: &JsDocTagType) -> bool {
        match ty {
            JsDocTagType::None => false,
            JsDocTagType::Literal { is_array, .. } => *is_array,
            JsDocTagType::Expr(expr) if expr.is_optional || expr.ty.is_none() => false,
            JsDocTagType::Expr(expr) if expr.is_variadic => true,
            JsDocTagType::Expr(expr) => match expr.ty.as_ref().map(|ty| &ty.kind) {
                Some(k::TypeExprKind::Array(_)) => true,
                Some(k::TypeExprKind::Reference { name, .. }) => {
                    name.len() == 1 && matches!(name[0].as_str(), "Array" | "ReadonlyArray")
                }
                _ => false,
            },
        }
    }
}

/// A diagnostic of a comment, reported as `kind`.
fn to_diagnostic(d: &JsDocDiagnostic, kind: b::DiagnosticKind) -> b::Diagnostic {
    let args: Vec<&[u8]> = d.args.iter().map(|arg| arg.as_bytes()).collect();
    let end = if d.end > d.start {
        d.end
    } else {
        b::Diagnostic::NO_LENGTH
    };
    b::Diagnostic::new(kind, (d.start, end), d.code, &args)
}

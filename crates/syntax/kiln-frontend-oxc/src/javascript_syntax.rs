//! What TypeScript reports in a JavaScript file that is written in TypeScript.
//!
//! TypeScript parses every JavaScript file as TypeScript, so a `.js` file with a type annotation
//! has a tree, and `getJSSyntacticDiagnosticsForFile` (`checkJSSyntax`) walks it to report each
//! construct that only TypeScript has (TS8002 to TS8038). Oxc rejects such a file as JavaScript;
//! parsed as TypeScript it is accepted, and this walk reports the same constructs at the same
//! places.

use kiln_hir::TypeJavaScriptError;
use oxc_ast::ast::{
    AccessorProperty, ArrowFunctionExpression, CallExpression, CatchParameter, Class,
    ExportAllDeclaration, ExportDeclaration, ExportFromDeclaration, ExportNamedDeclaration,
    FormalParameter, FormalParameterRest, Function, FunctionType, ImportDeclaration,
    ImportOrExportKind, JSXOpeningElement, MethodDefinition, NewExpression, PropertyDefinition,
    TSAsExpression, TSEnumDeclaration, TSExportAssignment, TSImportEqualsDeclaration,
    TSInterfaceDeclaration, TSNamespaceDeclaration, TSNamespaceDeclarationKind,
    TSNonNullExpression, TSSatisfiesExpression, TSTypeAliasDeclaration, TSTypeAnnotation,
    TSTypeParameterDeclaration, TSTypeParameterInstantiation, TaggedTemplateExpression,
    VariableDeclaration, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use std::collections::HashMap;

/// The modifiers written before a declaration that a JavaScript file may not have. `ModifierFlagsJavaScript`
/// is `export`, `static`, `accessor`, `async` and `default`; the rest of the modifiers are these.
const TYPESCRIPT_MODIFIERS: &[&str] = &[
    "public",
    "private",
    "protected",
    "readonly",
    "abstract",
    "declare",
    "override",
    "in",
    "out",
];

/// Every word that can come before a declaration's name as a modifier.
const MODIFIER_WORDS: &[&str] = &[
    "public",
    "private",
    "protected",
    "readonly",
    "abstract",
    "declare",
    "override",
    "in",
    "out",
    "export",
    "default",
    "static",
    "accessor",
    "async",
];

pub(crate) fn errors(
    program: &oxc_ast::ast::Program<'_>,
    source: &str,
) -> Vec<TypeJavaScriptError> {
    let mut walk = JavaScriptSyntax {
        source,
        errors: Vec::new(),
        exports: HashMap::new(),
    };
    walk.visit_program(program);
    let mut errors = walk.errors;
    errors.sort_by_key(|error| error.start);
    errors
}

struct JavaScriptSyntax<'s> {
    source: &'s str,
    errors: Vec<TypeJavaScriptError>,
    /// The start of the `export` statement around the declaration that starts at the key.
    exports: HashMap<u32, u32>,
}

impl JavaScriptSyntax<'_> {
    fn error(&mut self, start: u32, end: u32, code: u32, argument: Option<&'static str>) {
        self.errors.push(TypeJavaScriptError {
            start,
            end,
            code,
            argument,
        });
    }

    fn at(&mut self, span: Span, code: u32) {
        self.error(span.start, span.end, code, None);
    }

    /// `checkJSSyntax` for a type annotation: the type it names.
    fn annotation(&mut self, annotation: &Option<oxc_allocator::Box<'_, TSTypeAnnotation<'_>>>) {
        if let Some(annotation) = annotation {
            self.at(annotation.type_annotation.span(), 8010);
        }
    }

    fn type_parameters(
        &mut self,
        parameters: &Option<oxc_allocator::Box<'_, TSTypeParameterDeclaration<'_>>>,
    ) {
        if let Some(parameters) = parameters
            && let (Some(first), Some(last)) = (parameters.params.first(), parameters.params.last())
        {
            self.error(first.span.start, last.span.end, 8004, None);
        }
    }

    fn type_arguments(
        &mut self,
        arguments: &Option<oxc_allocator::Box<'_, TSTypeParameterInstantiation<'_>>>,
    ) {
        if let Some(arguments) = arguments
            && let (Some(first), Some(last)) = (arguments.params.first(), arguments.params.last())
        {
            self.error(first.span().start, last.span().end, 8011, None);
        }
    }

    /// The modifiers written in `from..to` that are TypeScript's: where each is, and its text.
    fn keywords(&self, from: u32, to: u32) -> Vec<(u32, &'static str)> {
        let bytes = self.source.as_bytes();
        let (mut at, to) = (from as usize, (to as usize).min(bytes.len()));
        let mut found = Vec::new();
        while at < to {
            match bytes[at] {
                b' ' | b'\t' | b'\r' | b'\n' => at += 1,
                b'/' if bytes.get(at + 1) == Some(&b'/') => {
                    while at < to && bytes[at] != b'\n' {
                        at += 1;
                    }
                }
                b'/' if bytes.get(at + 1) == Some(&b'*') => {
                    at += 2;
                    while at < to && !(bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/')) {
                        at += 1;
                    }
                    at += 2;
                }
                byte if byte.is_ascii_alphabetic() => {
                    let start = at;
                    while at < to && bytes[at].is_ascii_alphabetic() {
                        at += 1;
                    }
                    let word = &self.source[start..at];
                    if let Some(&word) = MODIFIER_WORDS.iter().find(|&&it| it == word) {
                        if TYPESCRIPT_MODIFIERS.contains(&word) {
                            found.push((start as u32, word));
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        found
    }

    /// `checkJSSyntax` for modifiers: those in `from..to` that a JavaScript file does not have.
    fn modifiers(&mut self, from: u32, to: u32) {
        for (start, word) in self.keywords(from, to) {
            self.error(start, 0, 8009, Some(word));
        }
    }

    /// Where the `?` after `end` is, if there is one.
    fn question_after(&self, end: u32) -> Option<u32> {
        let bytes = self.source.as_bytes();
        let mut at = end as usize;
        while let Some(&byte) = bytes.get(at) {
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' | b']' => at += 1,
                b'?' => return Some(at as u32),
                _ => return None,
            }
        }
        None
    }

    fn optional(&mut self, end: u32) {
        if let Some(question) = self.question_after(end) {
            self.error(question, 0, 8009, Some("?"));
        }
    }

    /// The start of what follows the decorators in `decorators`, or `start` for none.
    fn after_decorators(start: u32, decorators: &[oxc_ast::ast::Decorator<'_>]) -> u32 {
        decorators.last().map_or(start, |it| it.span.end)
    }

    /// `checkJSSyntax` for a statement that is a declaration, which may be written after `export`.
    fn statement_start(&self, declaration: Span) -> u32 {
        self.exports
            .get(&declaration.start)
            .copied()
            .unwrap_or(declaration.start)
    }
}

impl<'a> Visit<'a> for JavaScriptSyntax<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if it.import_kind == ImportOrExportKind::Type {
            self.error(it.span.start, it.span.end, 8006, Some("import type"));
        }
        walk::walk_import_declaration(self, it);
    }

    fn visit_export_declaration(&mut self, it: &ExportDeclaration<'a>) {
        self.exports
            .insert(it.declaration.span().start, it.span.start);
        walk::walk_export_declaration(self, it);
    }

    fn visit_export_named_declaration(&mut self, it: &ExportNamedDeclaration<'a>) {
        if it.export_kind == ImportOrExportKind::Type {
            self.error(it.span.start, it.span.end, 8006, Some("export type"));
        }
        walk::walk_export_named_declaration(self, it);
    }

    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) {
        if it.export_kind == ImportOrExportKind::Type {
            self.error(it.span.start, it.span.end, 8006, Some("export type"));
        }
        walk::walk_export_from_declaration(self, it);
    }

    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) {
        if it.export_kind == ImportOrExportKind::Type {
            self.error(it.span.start, it.span.end, 8006, Some("export type"));
        }
        walk::walk_export_all_declaration(self, it);
    }

    fn visit_export_default_declaration(
        &mut self,
        it: &oxc_ast::ast::ExportDefaultDeclaration<'a>,
    ) {
        self.exports
            .insert(it.declaration.span().start, it.span.start);
        walk::walk_export_default_declaration(self, it);
    }

    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        self.at(it.span, 8002);
        walk::walk_ts_import_equals_declaration(self, it);
    }

    fn visit_ts_export_assignment(&mut self, it: &TSExportAssignment<'a>) {
        self.at(it.span, 8003);
        walk::walk_ts_export_assignment(self, it);
    }

    fn visit_ts_interface_declaration(&mut self, it: &TSInterfaceDeclaration<'a>) {
        self.error(it.id.span.start, 0, 8006, Some("interface"));
        walk::walk_ts_interface_declaration(self, it);
    }

    fn visit_ts_enum_declaration(&mut self, it: &TSEnumDeclaration<'a>) {
        self.error(it.id.span.start, 0, 8006, Some("enum"));
        walk::walk_ts_enum_declaration(self, it);
    }

    fn visit_ts_namespace_declaration(&mut self, it: &TSNamespaceDeclaration<'a>) {
        let word = match it.kind {
            TSNamespaceDeclarationKind::Module => "module",
            TSNamespaceDeclarationKind::Namespace => "namespace",
        };
        self.error(it.id.span.start, 0, 8006, Some(word));
        walk::walk_ts_namespace_declaration(self, it);
    }

    fn visit_ts_type_alias_declaration(&mut self, it: &TSTypeAliasDeclaration<'a>) {
        self.error(it.id.span.start, 0, 8008, None);
        walk::walk_ts_type_alias_declaration(self, it);
    }

    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        if let Some(first) = it.declarations.first() {
            let start = self.statement_start(it.span);
            self.modifiers(start, first.span.start);
        }
        walk::walk_variable_declaration(self, it);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        self.annotation(&it.type_annotation);
        walk::walk_variable_declarator(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let declare = it.r#type == FunctionType::TSDeclareFunction;
        let start = self.statement_start(it.span);
        if let Some(id) = &it.id {
            self.modifiers(start, id.span.start);
        }
        self.type_parameters(&it.type_parameters);
        if let Some(this) = &it.this_param {
            self.annotation(&this.type_annotation);
        }
        if it.body.is_some() || !declare {
            self.annotation(&it.return_type);
        }
        if it.body.is_none() && declare {
            self.error(start, it.span.end, 8017, None);
        }
        walk::walk_function(self, it, flags);
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.type_parameters(&it.type_parameters);
        self.annotation(&it.return_type);
        walk::walk_arrow_function_expression(self, it);
    }

    fn visit_formal_parameter(&mut self, it: &FormalParameter<'a>) {
        let from = Self::after_decorators(it.span.start, &it.decorators);
        let pattern = it.pattern.span();
        if it.optional {
            self.optional(pattern.end);
        }
        let written = self.keywords(from, pattern.start);
        if let Some(&(last, word)) = written.last() {
            self.error(it.span.start, last + word.len() as u32, 8012, None);
        }
        self.annotation(&it.type_annotation);
        walk::walk_formal_parameter(self, it);
    }

    fn visit_formal_parameter_rest(&mut self, it: &FormalParameterRest<'a>) {
        self.annotation(&it.type_annotation);
        walk::walk_formal_parameter_rest(self, it);
    }

    fn visit_catch_parameter(&mut self, it: &CatchParameter<'a>) {
        self.annotation(&it.type_annotation);
        walk::walk_catch_parameter(self, it);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        let start = self.statement_start(it.span);
        if let Some(id) = &it.id {
            self.modifiers(start, id.span.start);
        } else if it.r#abstract || it.declare {
            self.modifiers(start, it.span.end);
        }
        self.type_parameters(&it.type_parameters);
        if let Some(heritage) = &it.heritage {
            self.type_arguments(&heritage.type_arguments);
        }
        if let Some(first) = it.implements.first() {
            let end = it
                .implements
                .last()
                .map_or(first.span.end, |it| it.span.end);
            let before = self.source[..first.span.start as usize].trim_end();
            let keyword = before.len().saturating_sub("implements".len()) as u32;
            self.error(keyword, end, 8005, None);
        }
        walk::walk_class(self, it);
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        let from = Self::after_decorators(it.span.start, &it.decorators);
        self.modifiers(from, it.key.span().start);
        if it.optional {
            self.optional(it.key.span().end);
        }
        if it.value.body.is_none() {
            self.error(it.span.start, it.span.end, 8017, None);
        }
        walk::walk_method_definition(self, it);
    }

    fn visit_property_definition(&mut self, it: &PropertyDefinition<'a>) {
        let from = Self::after_decorators(it.span.start, &it.decorators);
        self.modifiers(from, it.key.span().start);
        if it.optional {
            self.optional(it.key.span().end);
        }
        self.annotation(&it.type_annotation);
        walk::walk_property_definition(self, it);
    }

    fn visit_accessor_property(&mut self, it: &AccessorProperty<'a>) {
        let from = Self::after_decorators(it.span.start, &it.decorators);
        self.modifiers(from, it.key.span().start);
        self.annotation(&it.type_annotation);
        walk::walk_accessor_property(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.type_arguments(&it.type_arguments);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.type_arguments(&it.type_arguments);
        walk::walk_new_expression(self, it);
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        self.type_arguments(&it.type_arguments);
        walk::walk_tagged_template_expression(self, it);
    }

    fn visit_jsx_opening_element(&mut self, it: &JSXOpeningElement<'a>) {
        self.type_arguments(&it.type_arguments);
        walk::walk_jsx_opening_element(self, it);
    }

    fn visit_ts_non_null_expression(&mut self, it: &TSNonNullExpression<'a>) {
        self.at(it.span, 8013);
        walk::walk_ts_non_null_expression(self, it);
    }

    fn visit_ts_as_expression(&mut self, it: &TSAsExpression<'a>) {
        self.at(it.type_annotation.span(), 8016);
        walk::walk_ts_as_expression(self, it);
    }

    fn visit_ts_satisfies_expression(&mut self, it: &TSSatisfiesExpression<'a>) {
        self.at(it.type_annotation.span(), 8037);
        walk::walk_ts_satisfies_expression(self, it);
    }
}

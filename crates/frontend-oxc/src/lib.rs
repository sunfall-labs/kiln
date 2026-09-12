//! Oxc adapter for Kiln's owned HIR.
//!
//! This is the only crate allowed to expose Oxc to the compiler build graph. Its public API
//! accepts ordinary strings and returns [`kiln_hir`] values, so an Oxc upgrade cannot leak AST
//! types or arena lifetimes into the query database, interpreter, or backends.

use std::{collections::HashSet, path::Path, sync::Arc};

use kiln_hir::{
    AssignmentOperator as HirAssignmentOperator, BinaryOperator as HirBinaryOperator, Diagnostic,
    Export, ExportName, ExportOrigin, Expression as HirExpression,
    ExpressionKind as HirExpressionKind, Function as HirFunction, Import, ImportName, Item,
    ItemKind, ListElement, Literal as HirLiteral, LogicalOperator as HirLogicalOperator, Module,
    ModuleRequestKind, ModuleRequestOccurrence, Parameter as HirParameter, RequestedModule,
    SourceGoal, SourceKind, Span as HirSpan, Statement as HirStatement,
    StatementKind as HirStatementKind, TypeAnnotation as HirTypeAnnotation,
    UnaryOperator as HirUnaryOperator, VariableDeclarator as HirVariableDeclarator,
    VariableKind as HirVariableKind,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpressionElement, ForStatementInit, ForStatementLeft, SimpleAssignmentTarget,
};
use oxc_ast::{
    AstKind,
    ast::{
        Argument, ArrowFunctionBody, ArrowFunctionExpression, AssignmentTarget, BindingPattern,
        Class, ClassElement, Declaration, Directive, ExportDefaultDeclarationKind, Expression,
        FormalParameters, Function, ImportDeclarationSpecifier, MethodDefinitionKind,
        ObjectExpression, ObjectPropertyKind, PropertyKind, Statement, VariableDeclaration,
        VariableDeclarationKind,
    },
};
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::{IsGlobalReference, SemanticBuilder};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::module_record::{
    ExportEntry as OxcExportEntry, ExportExportName, ExportImportName, ExportLocalName,
    ImportImportName,
};
use oxc_syntax::operator::UpdateOperator;
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};
use oxc_transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer};

/// Parse one JavaScript or TypeScript module and lower its module-level structure into Kiln HIR.
///
/// Parser diagnostics are data, not a transport error: Oxc can recover and produce a useful
/// module after many syntax errors, and the query database must cache that result just like a
/// clean parse. An unknown extension falls back to module-mode JavaScript; CLI source discovery
/// already restricts ordinary project files to known JS/TS extensions.
pub fn parse(path: &str, source: &str) -> Module {
    let mut source_type = SourceType::from_path(Path::new(path))
        .unwrap_or_else(|_| SourceType::default().with_module(true));
    if !path.ends_with(".cjs") {
        source_type = source_type.with_module(true);
    }
    parse_source(Some(path), source, source_type)
}

/// Parse with an explicit ECMAScript goal, independent of Oxc's extension convention.
///
/// Test262 uses `.js` for both scripts and modules, so path inference alone is insufficient for
/// conformance work. The language variant (JS/TS/JSX) still comes from the path.
pub fn parse_with_goal(path: &str, source: &str, goal: SourceGoal) -> Module {
    let inferred = SourceType::from_path(Path::new(path)).unwrap_or_default();
    let source_type = match goal {
        SourceGoal::Script => inferred.with_script(true),
        SourceGoal::Module => inferred.with_module(true),
    };
    parse_source(Some(path), source, source_type)
}

/// What the code around a direct `eval` makes legal inside it.
///
/// Eval code is parsed as a Script, and a Script alone may not write `super.x`, `new.target` or a
/// private name -- so the three early errors that say so are exactly wrong for source that was
/// handed to `eval` from inside a method, a function, or a class body. The specification says as
/// much: PerformEval applies the eval rules "as outside a constructor, inside a method, and
/// inside a function" according to where the call was written.
///
/// The permission is per-construct and per-name rather than a blanket "this came from eval":
/// `eval('super.x')` written at the top level of a script is still a `SyntaxError`, and so is
/// `eval('this.#absent')` for a name no enclosing class declared. Anything not named here is
/// reported exactly as it was before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalSyntax {
    /// `new.target` is legal: the call is inside a function, or a class field initialiser.
    pub new_target: bool,
    /// `super.x` and `super[e]` are legal: the call is inside a method or a field initialiser.
    pub super_property: bool,
    /// The private names in scope, spelled without the `#`.
    pub private_names: Vec<String>,
}

impl EvalSyntax {
    /// Whether this context makes one reported early error not an error at all.
    ///
    /// Matched on the message because Oxc gives these diagnostics no code to match on instead.
    /// That is a pin to one Oxc version, and it is checked rather than trusted: the tests below
    /// assert that each of these three suppressions actually fires, so a reworded upstream
    /// diagnostic fails the build rather than silently returning to rejecting legal code.
    fn permits(&self, message: &str) -> bool {
        if self.new_target && message == "Unexpected new.target expression" {
            return true;
        }
        if self.super_property
            && message
                == "'super' can only be referenced in members of derived classes or object literal expressions."
        {
            return true;
        }
        self.private_names.iter().any(|name| {
            message == format!("Private identifier '#{name}' is not allowed outside class bodies")
        })
    }
}

/// Parse the source handed to a direct `eval`, in the syntactic context of its call site.
///
/// Always the Script goal, like any other eval: `allow` widens which early errors apply, and
/// nothing else.
pub fn parse_eval(path: &str, source: &str, allow: &EvalSyntax) -> Module {
    let inferred = SourceType::from_path(Path::new(path)).unwrap_or_default();
    parse_source_in_context(Some(path), source, inferred.with_script(true), Some(allow))
}

/// Parse a CommonJS file with Oxc's explicit wrapper-aware goal while preserving the owned HIR
/// boundary. Package-format policy belongs to the Node resolver/graph; the frontend only selects
/// grammar. Unlike an ordinary Script goal, this admits the wrapper's top-level `return` and
/// `new.target` while continuing to reject top-level `await`.
pub fn parse_commonjs(path: &str, source: &str) -> Module {
    let inferred = SourceType::from_path(Path::new(path)).unwrap_or_default();
    parse_source(Some(path), source, inferred.with_commonjs(true))
}

/// Parse CommonJS and return the literal dependencies identified by Oxc's own scope graph.
///
/// Keeping this fact beside parsing avoids reimplementing JavaScript hoisting and shadowing in
/// Kiln HIR. Only owned strings cross the frontend boundary; Oxc nodes and arena lifetimes do not.
pub fn parse_commonjs_with_requires(path: &str, source: &str) -> (Module, Vec<String>) {
    let inferred = SourceType::from_path(Path::new(path)).unwrap_or_default();
    parse_source_with_commonjs_requires(Some(path), source, inferred.with_commonjs(true))
}

fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;

    while i < chars.len() {
        if in_string {
            out.push(chars[i]);
            if escape {
                escape = false;
            } else if chars[i] == '\\' {
                escape = true;
            } else if chars[i] == '"' {
                in_string = false;
            }
            i += 1;
        } else {
            if chars[i] == '"' {
                in_string = true;
                out.push(chars[i]);
                i += 1;
            } else if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '/' {
                    i += 2;
                } else {
                    i = chars.len();
                }
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn strip_trailing_commas(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;

    while i < chars.len() {
        if in_string {
            out.push(chars[i]);
            if escape {
                escape = false;
            } else if chars[i] == '\\' {
                escape = true;
            } else if chars[i] == '"' {
                in_string = false;
            }
            i += 1;
        } else {
            if chars[i] == '"' {
                in_string = true;
                out.push(chars[i]);
                i += 1;
            } else if chars[i] == ',' {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                    i += 1;
                } else {
                    out.push(',');
                    i += 1;
                }
            } else {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn parse_tsconfig_jsx(content: &str) -> Option<JsxOptions> {
    let stripped = strip_trailing_commas(&strip_jsonc_comments(content));
    let value: serde_json::Value = serde_json::from_str(&stripped).ok()?;
    let compiler_options = value.get("compilerOptions")?;

    let jsx_mode = compiler_options.get("jsx").and_then(|v| v.as_str());
    let import_source = compiler_options
        .get("jsxImportSource")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pragma = compiler_options
        .get("jsxFactory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let pragma_frag = compiler_options
        .get("jsxFragmentFactory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if jsx_mode.is_none() && import_source.is_none() && pragma.is_none() && pragma_frag.is_none() {
        return None;
    }

    let (runtime, development) = match jsx_mode {
        Some("react") => (JsxRuntime::Classic, false),
        Some("react-jsx") => (JsxRuntime::Automatic, false),
        Some("react-jsxdev") => (JsxRuntime::Automatic, true),
        Some("preserve") => (JsxRuntime::Automatic, false),
        _ => (JsxRuntime::Automatic, false),
    };

    Some(JsxOptions {
        jsx_plugin: true,
        runtime,
        development,
        import_source,
        pragma,
        pragma_frag,
        ..JsxOptions::default()
    })
}

fn resolve_jsx_options(path: Option<&str>) -> JsxOptions {
    if let Some(path_str) = path {
        let p = Path::new(path_str);
        let full_path = if p.is_absolute() {
            p.to_path_buf()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(p)
        } else {
            p.to_path_buf()
        };

        let mut current_dir = full_path.parent();
        while let Some(dir) = current_dir {
            let tsconfig = dir.join("tsconfig.json");
            if let Some(options) = std::fs::read_to_string(&tsconfig)
                .ok()
                .and_then(|c| parse_tsconfig_jsx(&c))
            {
                return options;
            }
            let jsconfig = dir.join("jsconfig.json");
            if let Some(options) = std::fs::read_to_string(&jsconfig)
                .ok()
                .and_then(|c| parse_tsconfig_jsx(&c))
            {
                return options;
            }
            current_dir = dir.parent();
        }
    }

    JsxOptions {
        jsx_plugin: true,
        runtime: JsxRuntime::Automatic,
        ..JsxOptions::default()
    }
}

fn resolve_transform_options(path: Option<&str>) -> TransformOptions {
    TransformOptions {
        jsx: resolve_jsx_options(path),
        ..TransformOptions::default()
    }
}

fn parse_source_with_commonjs_requires(
    path: Option<&str>,
    source: &str,
    source_type: SourceType,
) -> (Module, Vec<String>) {
    let allocator = Allocator::default();
    let mut parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();

    if source_type.is_jsx() && !parsed.panicked && parsed.diagnostics.is_empty() {
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let scoping = semantic.into_scoping();
        let transform_options = resolve_transform_options(path);
        let source_path = path
            .map(Path::new)
            .unwrap_or_else(|| Path::new("inline.tsx"));
        let ret = Transformer::new(&allocator, source_path, &transform_options)
            .build_with_scoping(scoping, &mut parsed.program);
        parsed.diagnostics.extend(ret.diagnostics);
    }

    let requires = if parsed.panicked || !parsed.diagnostics.is_empty() {
        Vec::new()
    } else {
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program)
            .semantic;
        let mut requires = semantic
            .nodes()
            .iter()
            .filter_map(|node| {
                let AstKind::CallExpression(call) = node.kind() else {
                    return None;
                };
                let Expression::Identifier(callee) = &call.callee else {
                    return None;
                };
                if callee.name != "require" || !callee.is_global_reference(semantic.scoping()) {
                    return None;
                }
                let argument = call.arguments.first()?.as_expression()?;
                let Expression::StringLiteral(literal) = argument else {
                    return None;
                };
                Some((call.span.start, literal.value.to_string()))
            })
            .collect::<Vec<_>>();
        requires.sort_by_key(|(start, _)| *start);
        let mut seen = HashSet::new();
        requires
            .into_iter()
            .filter_map(|(_, value)| seen.insert(value.clone()).then_some(value))
            .collect()
    };
    (lower_parsed(path, &parsed, source, source_type), requires)
}

fn parse_source(path: Option<&str>, source: &str, source_type: SourceType) -> Module {
    parse_source_in_context(path, source, source_type, None)
}

fn parse_source_in_context(
    path: Option<&str>,
    source: &str,
    source_type: SourceType,
    allow: Option<&EvalSyntax>,
) -> Module {
    let allocator = Allocator::default();
    // A regular expression literal's *pattern* is a grammar of its own, and Oxc only runs it when
    // asked: without this the tokenizer checks the flags, hands back the pattern as opaque text,
    // and `/(/` or `/\\p{ASCII=F}/u` parse clean.  Those are `SyntaxError`s at parse time, before
    // any code runs, so they have to be decided here rather than when the regexp is first
    // constructed.  Pattern errors are raised through the parser's own error channel, so they
    // arrive in `parsed.diagnostics` and need no separate fold.
    let mut parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();

    if source_type.is_jsx() && !parsed.panicked && parsed.diagnostics.is_empty() {
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let scoping = semantic.into_scoping();
        let transform_options = resolve_transform_options(path);
        let source_path = path
            .map(Path::new)
            .unwrap_or_else(|| Path::new("inline.tsx"));
        let ret = Transformer::new(&allocator, source_path, &transform_options)
            .build_with_scoping(scoping, &mut parsed.program);
        parsed.diagnostics.extend(ret.diagnostics);
    }

    lower_parsed_in_context(path, &parsed, source, source_type, allow)
}

fn lower_parsed(
    path: Option<&str>,
    parsed: &oxc_parser::ParserReturn<'_>,
    source: &str,
    source_type: SourceType,
) -> Module {
    lower_parsed_in_context(path, parsed, source, source_type, None)
}

fn lower_parsed_in_context(
    path: Option<&str>,
    parsed: &oxc_parser::ParserReturn<'_>,
    source: &str,
    source_type: SourceType,
    allow: Option<&EvalSyntax>,
) -> Module {
    let mut requested_modules = parsed
        .module_record
        .requested_modules
        .iter()
        .map(|(specifier, occurrences)| RequestedModule {
            specifier: specifier.to_string(),
            occurrences: occurrences
                .iter()
                .map(|occurrence| ModuleRequestOccurrence {
                    statement_span: span(occurrence.statement_span),
                    specifier_span: span(occurrence.span),
                    type_only: occurrence.is_type,
                    kind: if occurrence.is_import {
                        ModuleRequestKind::Import
                    } else {
                        ModuleRequestKind::Export
                    },
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let mut imports = parsed
        .module_record
        .import_entries
        .iter()
        .map(|entry| Import {
            source: entry.module_request.name.to_string(),
            imported: lower_import_name(&entry.import_name),
            local: entry.local_name.name.to_string(),
            type_only: entry.is_type,
            span: span(entry.local_name.span),
            statement_span: span(entry.statement_span),
        })
        .collect::<Vec<_>>();

    for stmt in &parsed.program.body {
        if let Statement::ImportDeclaration(decl) = stmt {
            let specifier = decl.source.value.as_str();
            let is_type = decl.import_kind.is_type();

            if let Some(req) = requested_modules
                .iter_mut()
                .find(|r| r.specifier == specifier)
            {
                if req.occurrences.is_empty() {
                    req.occurrences.push(ModuleRequestOccurrence {
                        statement_span: span(decl.span),
                        specifier_span: span(decl.source.span),
                        type_only: is_type,
                        kind: ModuleRequestKind::Import,
                    });
                }
            } else {
                requested_modules.push(RequestedModule {
                    specifier: specifier.to_string(),
                    occurrences: vec![ModuleRequestOccurrence {
                        statement_span: span(decl.span),
                        specifier_span: span(decl.source.span),
                        type_only: is_type,
                        kind: ModuleRequestKind::Import,
                    }],
                });
            }

            if let Some(specifiers) = &decl.specifiers {
                for spec in specifiers {
                    let (imported, local, type_only, spec_span) = match spec {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => (
                            ImportName::Name(s.imported.name().to_string()),
                            s.local.name.to_string(),
                            is_type || s.import_kind.is_type(),
                            s.span,
                        ),
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => (
                            ImportName::Default,
                            s.local.name.to_string(),
                            is_type,
                            s.span,
                        ),
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => (
                            ImportName::Namespace,
                            s.local.name.to_string(),
                            is_type,
                            s.span,
                        ),
                    };

                    if !imports
                        .iter()
                        .any(|i| i.source == specifier && i.local == local)
                    {
                        imports.push(Import {
                            source: specifier.to_string(),
                            imported,
                            local,
                            type_only,
                            span: span(spec_span),
                            statement_span: span(decl.span),
                        });
                    }
                }
            }
        }
    }

    requested_modules.sort_by_key(|request| {
        request
            .occurrences
            .first()
            .map(|occurrence| occurrence.statement_span)
            .unwrap_or_default()
            .start
    });
    imports.sort_by_key(|entry| (entry.statement_span.start, entry.span.start));

    // Use the spec-shaped entries rather than `exported_bindings`: the latter intentionally
    // excludes `default` and erases TypeScript's type-only bit, both of which affect Kiln's
    // runtime interface.
    let mut exports = parsed
        .module_record
        .local_export_entries
        .iter()
        .chain(parsed.module_record.indirect_export_entries.iter())
        .map(lower_export)
        .collect::<Vec<_>>();

    for entry in &parsed.module_record.star_export_entries {
        exports.push(lower_export(entry));
    }
    exports.sort_by_key(|export| export.span.start);

    let mut diagnostics = parsed
        .diagnostics
        .iter()
        .map(lower_diagnostic)
        .collect::<Vec<_>>();
    // A permitted early error is dropped *before* the semantic pass is decided on, not after:
    // `new.target` is refused by the parser itself, and leaving its diagnostic in place would
    // suppress every other early error in the same source rather than only this one.
    if let Some(allow) = allow {
        diagnostics.retain(|diagnostic| !allow.permits(&diagnostic.message));
    }
    if diagnostics.is_empty() {
        let mut early = early_errors(parsed);
        if let Some(allow) = allow {
            early.retain(|diagnostic| !allow.permits(&diagnostic.message));
        }
        diagnostics.extend(early);
    }
    let mut lowerer = Lowerer { text: source, path };
    let body = lowerer.statements(&parsed.program.body);

    let mut module = Module {
        // The one copy of the text every span in this module points into.
        source: Arc::from(source),
        source_kind: source_kind(source_type),
        source_goal: if source_type.is_module() {
            SourceGoal::Module
        } else {
            SourceGoal::Script
        },
        strict: opens_strict(&parsed.program.directives),
        items: parsed
            .program
            .body
            .iter()
            .map(|statement| Item {
                kind: item_kind(statement),
                span: span(statement.span()),
            })
            .collect(),
        body,
        requested_modules,
        imports,
        exports,
        diagnostics,
        panicked: parsed.panicked,
    };
    module.check_isolate_escapes();
    module
}

/// Whether a directive prologue opens with `"use strict"`.
///
/// The *raw* text is what counts, not the string the literal denotes: `"use\u0020strict"` spells
/// the same characters and is deliberately not a directive, because the language reads a prologue
/// before it has evaluated anything -- including an escape. Oxc keeps both spellings, so this
/// reads the one the source wrote.
fn opens_strict(directives: &[Directive<'_>]) -> bool {
    directives
        .iter()
        .any(|directive| directive.directive.as_str() == "use strict")
}

/// Lower a TypeScript type annotation into a coarse HIR type annotation.
///
/// Order of trusted annotations follows Static Hermes's published results:
/// (number, int, boolean, string).
fn lower_type_annotation(
    annotation: &oxc_ast::ast::TSTypeAnnotation<'_>,
) -> Option<HirTypeAnnotation> {
    lower_ts_type(&annotation.type_annotation)
}

fn lower_ts_type(ts_type: &oxc_ast::ast::TSType<'_>) -> Option<HirTypeAnnotation> {
    use oxc_ast::ast::{TSType, TSTypeName};
    match ts_type {
        TSType::TSNumberKeyword(_) => Some(HirTypeAnnotation::Number),
        TSType::TSBooleanKeyword(_) => Some(HirTypeAnnotation::Boolean),
        TSType::TSStringKeyword(_) => Some(HirTypeAnnotation::String),
        TSType::TSVoidKeyword(_) => Some(HirTypeAnnotation::Void),
        TSType::TSAnyKeyword(_) | TSType::TSUnknownKeyword(_) => Some(HirTypeAnnotation::Any),
        TSType::TSTypeReference(ref_type) => match &ref_type.type_name {
            TSTypeName::IdentifierReference(ident) => match ident.name.as_str() {
                "number" => Some(HirTypeAnnotation::Number),
                "int" | "int32" => Some(HirTypeAnnotation::Int),
                "boolean" | "bool" => Some(HirTypeAnnotation::Boolean),
                "string" => Some(HirTypeAnnotation::String),
                "void" => Some(HirTypeAnnotation::Void),
                "any" => Some(HirTypeAnnotation::Any),
                _ => Some(HirTypeAnnotation::Other),
            },
            _ => Some(HirTypeAnnotation::Other),
        },
        TSType::TSParenthesizedType(paren) => lower_ts_type(&paren.type_annotation),
        _ => Some(HirTypeAnnotation::Other),
    }
}

/// Lower one Oxc diagnostic into HIR, preferring the span Oxc marked primary.
///
/// Parser and semantic diagnostics carry the same label shape, so both go through here and a
/// reported early error points at source the same way a syntax error does.
fn lower_diagnostic(diagnostic: &OxcDiagnostic) -> Diagnostic {
    let diagnostic_span = diagnostic
        .labels
        .iter()
        .find(|label| label.primary())
        .or_else(|| diagnostic.labels.first())
        .map(|label| span(label.span()));
    Diagnostic {
        message: diagnostic.to_string(),
        span: diagnostic_span,
    }
}

/// Early errors, which the ECMAScript grammar defers to static semantics rather than the parser.
///
/// `(eval) = 20` under a strict directive, `let x; const x = 0;` in a module, and `arguments`
/// inside a class field initializer are all well-formed to a recursive-descent parser and all
/// required to be `SyntaxError`s before evaluation begins.  Oxc raises them from
/// [`SemanticBuilder`] rather than the parser, so a frontend that stops after `Parser::parse`
/// accepts thousands of programs the specification requires it to reject.
///
/// The goal and strict mode ride along in the [`SourceType`] the parser was given and in the
/// program's own directive prologue, which is why this runs on the parsed program rather than
/// re-deriving either: the same source is legal as a sloppy script and illegal as a module.
///
/// Nothing is checked once the parser itself has failed.  Semantic analysis of an error-recovered
/// AST reports errors about a program the author never wrote, and the source is already rejected,
/// so the cascade would only bury the diagnostic that matters.  The caller decides that: it runs
/// this only when no parser diagnostic survived, which for ordinary source means Oxc reported
/// none and for eval code means the only one it reported was the `new.target` the call site
/// makes legal.
fn early_errors(parsed: &oxc_parser::ParserReturn<'_>) -> Vec<Diagnostic> {
    if parsed.panicked {
        return Vec::new();
    }
    // `new_compiler` is the cheap configuration: syntax checks on, the random-access node store
    // off.  Only the diagnostics are wanted; scope and symbol tables are discarded.
    SemanticBuilder::new_compiler()
        .build(&parsed.program)
        .diagnostics
        .errors()
        .map(lower_diagnostic)
        .collect()
}

/// A BigInt literal's value as plain decimal digits.
///
/// `0x1fn`, `1_000n` and `10n` all reach the IR as decimal text, so the interpreter and the
/// program-hash never have to re-derive a base. Returns `None` for anything that does not parse,
/// which leaves the refusal in place rather than inventing a number.
fn bigint_digits(raw: &str) -> Option<String> {
    let text = raw.strip_suffix('n').unwrap_or(raw).replace('_', "");
    let (digits, radix) = match text.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => (&text[2..], 16),
        Some("0o") => (&text[2..], 8),
        Some("0b") => (&text[2..], 2),
        _ => (text.as_str(), 10),
    };
    if digits.is_empty() {
        return None;
    }
    Some(num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)?.to_string())
}

/// How many bytes of whitespace and comments `text` opens with.
///
/// A hand-rolled scan rather than a re-lex: the only caller needs the *end* of the trivia after
/// one keyword, and the parser has already proved the text is well formed -- so an unterminated
/// comment cannot arrive here, and one that somehow did stops at the end rather than running off
/// it.
fn skipped_trivia(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut at = 0;
    loop {
        let rest = &text[at..];
        let spaces = rest.len() - rest.trim_start().len();
        at += spaces;
        if text[at..].starts_with("/*") {
            match text[at + 2..].find("*/") {
                Some(close) => at += 2 + close + 2,
                None => return bytes.len(),
            }
        } else if text[at..].starts_with("//") {
            match text[at..].find('\n') {
                Some(newline) => at += newline + 1,
                None => return bytes.len(),
            }
        } else {
            return at;
        }
    }
}

struct Lowerer<'a> {
    /// The source being lowered, so a span can be *read* and not merely recorded.
    ///
    /// Only one thing needs this, and it needs it for a reason no span arithmetic can supply: a
    /// static class member's `ClassElement` span starts at `static`, and the `MethodDefinition`
    /// that `Function.prototype.toString` must answer with does not include it. Finding where the
    /// keyword ends means looking at the text.
    text: &'a str,
    path: Option<&'a str>,
}

impl Lowerer<'_> {
    /// A class member's definition span, which for a static member starts after the `static`.
    ///
    /// `class C { static s() {} }` answers `"s() {}"`: `static` belongs to the `ClassElement`,
    /// not to the `MethodDefinition` the specification says to reproduce. A member *named*
    /// `static` is unaffected -- `static static() {}` drops the first word and keeps the second,
    /// which is the same rule applied once.
    ///
    /// Whitespace *and comments* between the keyword and the key are skipped. Both are trivia
    /// belonging to neither production, and the corpus checks it directly:
    /// `class F { static /* before */f() {} }` must answer `"f() {}"`, and keeping the comment
    /// left a string the NativeFunction and MethodDefinition grammars both reject -- so twenty
    /// variants failed on the *text* rather than on anything about the method.
    fn method_definition_span(&self, span: HirSpan, is_static: bool) -> HirSpan {
        if !is_static {
            return span;
        }
        let Some(text) = self
            .text
            .get(span.start as usize..span.end as usize)
            .and_then(|text| text.strip_prefix("static"))
        else {
            return span;
        };
        HirSpan::new(
            span.start + "static".len() as u32 + skipped_trivia(text) as u32,
            span.end,
        )
    }

    /// Lower a list of statements into one vector, sized for the source list up front.
    ///
    /// Nothing here needs the capacity to be exact -- `statement` erases a type-only declaration
    /// and returns `None`, so the result can be shorter -- but it does need the vector to be
    /// allocated *once*. Grown by pushes instead, a list of n statements reallocates log n times
    /// and leaves a freed block of every intermediate size behind it, and an allocator that
    /// keeps those blocks rather than handing them back makes each successive parse of a large
    /// source dirty fresh pages instead of reusing the last one's. That is invisible for a
    /// program compiled once and fatal for `eval`, which compiles a fresh program every call:
    /// three evals of a four-megabyte source held a gigabyte that nothing pointed at.
    fn statements(&mut self, source: &[Statement<'_>]) -> Vec<HirStatement> {
        let mut lowered = Vec::with_capacity(source.len());
        lowered.extend(
            source
                .iter()
                .filter_map(|statement| self.statement(statement)),
        );
        lowered
    }

    fn statement(&mut self, statement: &Statement<'_>) -> Option<HirStatement> {
        let statement_span = span(statement.span());
        let kind = match statement {
            Statement::VariableDeclaration(declaration) => self.variable(declaration),
            Statement::ExportDeclaration(declaration) => {
                return self.declaration(&declaration.declaration, statement_span);
            }
            Statement::ExportDefaultDeclaration(declaration) => {
                if let Some(expression) = declaration.declaration.as_expression() {
                    HirStatementKind::ExportDefault(self.expression(expression))
                } else if matches!(
                    declaration.declaration,
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_)
                ) {
                    return None;
                } else if let ExportDefaultDeclarationKind::FunctionDeclaration(declaration) =
                    &declaration.declaration
                {
                    // `export default function f() {}` declares `f` and exports it under
                    // `default`; the anonymous form declares the name the module record already
                    // uses for it, which carries characters an identifier cannot.
                    let lowered = self
                        .function(declaration, statement_span)
                        .map(|mut function| {
                            function
                                .name
                                .get_or_insert_with(|| kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
                            HirStatementKind::Function(Box::new(function))
                        })
                        .ok_or_else(|| "default function export".to_string());
                    self.default_declaration(lowered, statement_span)
                } else if let ExportDefaultDeclarationKind::ClassDeclaration(declaration) =
                    &declaration.declaration
                {
                    let lowered = self.class(declaration, statement_span).map(|mut class| {
                        class
                            .name
                            .get_or_insert_with(|| kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
                        HirStatementKind::Class(Box::new(class))
                    });
                    self.default_declaration(lowered, statement_span)
                } else {
                    self.unsupported_statement("default function or class export", statement_span)
                }
            }
            Statement::ExpressionStatement(expression) => {
                HirStatementKind::Expression(self.expression(&expression.expression))
            }
            Statement::BlockStatement(block) => {
                HirStatementKind::Block(self.statements(&block.body))
            }
            Statement::IfStatement(statement) => HirStatementKind::If {
                test: self.expression(&statement.test),
                consequent: Box::new(self.statement_or_unsupported(&statement.consequent)),
                alternate: statement
                    .alternate
                    .as_ref()
                    .map(|alternate| Box::new(self.statement_or_unsupported(alternate))),
            },
            Statement::WhileStatement(statement) => HirStatementKind::While {
                test: self.expression(&statement.test),
                body: Box::new(self.statement_or_unsupported(&statement.body)),
            },
            Statement::LabeledStatement(statement) => HirStatementKind::Labelled {
                label: statement.label.name.to_string(),
                body: Box::new(self.statement_or_unsupported(&statement.body)),
            },
            Statement::EmptyStatement(_) => HirStatementKind::Empty,

            // Module linkage and erased TypeScript declarations have no local runtime action.
            Statement::ImportDeclaration(_)
            | Statement::ExportAllDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportFromDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_)
            | Statement::TSGlobalDeclaration(_)
            | Statement::TSNamespaceExportDeclaration(_) => return None,

            Statement::FunctionDeclaration(declaration) => {
                self.function_declaration(declaration, statement_span)
            }
            Statement::ClassDeclaration(declaration) => {
                match self.class(declaration, statement_span) {
                    Ok(class) => HirStatementKind::Class(Box::new(class)),
                    Err(feature) => self.unsupported_statement(&feature, statement_span),
                }
            }
            Statement::DoWhileStatement(statement) => HirStatementKind::DoWhile {
                body: Box::new(self.statement_or_unsupported(&statement.body)),
                test: self.expression(&statement.test),
            },
            Statement::ForStatement(statement) => HirStatementKind::For {
                init: statement
                    .init
                    .as_ref()
                    .and_then(|init| self.for_init(init, statement_span))
                    .map(Box::new),
                test: statement.test.as_ref().map(|test| self.expression(test)),
                update: statement
                    .update
                    .as_ref()
                    .map(|update| self.expression(update)),
                body: Box::new(self.statement_or_unsupported(&statement.body)),
            },
            Statement::ForOfStatement(statement) => {
                let ForStatementLeft::VariableDeclaration(declaration) = &statement.left else {
                    // `for (x of xs)` assigns to something that already exists instead of
                    // binding a name. Rather than give the loop a second head shape, bind a
                    // hidden name and let the body assign it across -- which is also the order
                    // the specification uses, taking the target's reference only once the step's
                    // value is in hand, so `for (o[i()] of xs)` calls `i` once per iteration.
                    //
                    // The hidden name is `let` so it is fresh each iteration: a closure the body
                    // makes over the target still sees whatever the target holds, but a
                    // destructuring default that captures the step's value must not see the last
                    // one.
                    let Some(target) = statement.left.as_assignment_target() else {
                        return Some(HirStatement {
                            kind: self.unsupported_statement("for-of head", statement_span),
                            span: statement_span,
                        });
                    };
                    // Derived from the span rather than a counter, because two heads in one
                    // source cannot start at the same offset, and `%` is not in the identifier
                    // grammar so no source can name -- or shadow -- this binding.
                    let hidden = format!("%forof{}", statement_span.start);
                    let step = HirExpression {
                        kind: HirExpressionKind::Identifier(hidden.clone()),
                        span: statement_span,
                    };
                    let assignment =
                        self.assign_to(target, HirAssignmentOperator::Assign, step, statement_span);
                    return Some(HirStatement {
                        kind: HirStatementKind::ForOf {
                            kind: HirVariableKind::Let,
                            pattern: kiln_hir::Pattern::Identifier(hidden),
                            iterable: self.expression(&statement.right),
                            body: self.assigning_loop_body(
                                assignment,
                                &statement.body,
                                statement_span,
                            ),
                            awaiting: statement.r#await,
                        },
                        span: statement_span,
                    });
                };
                let Some(declarator) = declaration.declarations.first() else {
                    return Some(HirStatement {
                        kind: self.unsupported_statement("for-of binding", statement_span),
                        span: statement_span,
                    });
                };
                let pattern = self.pattern(&declarator.id, statement_span);
                HirStatementKind::ForOf {
                    kind: match declaration.kind {
                        VariableDeclarationKind::Var => HirVariableKind::Var,
                        VariableDeclarationKind::Const => HirVariableKind::Const,
                        _ => HirVariableKind::Let,
                    },
                    pattern,
                    iterable: self.expression(&statement.right),
                    body: Box::new(self.statement_or_unsupported(&statement.body)),
                    awaiting: statement.r#await,
                }
            }
            Statement::ForInStatement(statement) => {
                let (kind, name) = match self.for_binding(&statement.left) {
                    Some(binding) => binding,
                    // `for (x in o)` is the same desugaring as the `for-of` above, for the same
                    // reason; a declaration `for_binding` rejected for some other cause -- a
                    // destructuring pattern, say -- is not an assignment target and still gets
                    // the diagnostic it had.
                    None => {
                        let Some(target) = statement.left.as_assignment_target() else {
                            return Some(HirStatement {
                                kind: self.unsupported_statement("for-in binding", statement_span),
                                span: statement_span,
                            });
                        };
                        let hidden = format!("%forin{}", statement_span.start);
                        let step = HirExpression {
                            kind: HirExpressionKind::Identifier(hidden.clone()),
                            span: statement_span,
                        };
                        let assignment = self.assign_to(
                            target,
                            HirAssignmentOperator::Assign,
                            step,
                            statement_span,
                        );
                        return Some(HirStatement {
                            kind: HirStatementKind::ForIn {
                                kind: HirVariableKind::Let,
                                name: hidden,
                                object: self.expression(&statement.right),
                                body: self.assigning_loop_body(
                                    assignment,
                                    &statement.body,
                                    statement_span,
                                ),
                            },
                            span: statement_span,
                        });
                    }
                };
                HirStatementKind::ForIn {
                    kind,
                    name,
                    object: self.expression(&statement.right),
                    body: Box::new(self.statement_or_unsupported(&statement.body)),
                }
            }
            Statement::BreakStatement(statement) => HirStatementKind::Break {
                label: statement.label.as_ref().map(|label| label.name.to_string()),
            },
            Statement::ContinueStatement(statement) => HirStatementKind::Continue {
                label: statement.label.as_ref().map(|label| label.name.to_string()),
            },
            Statement::ReturnStatement(statement) => HirStatementKind::Return(
                statement
                    .argument
                    .as_ref()
                    .map(|value| self.expression(value)),
            ),
            Statement::SwitchStatement(statement) => HirStatementKind::Switch {
                discriminant: self.expression(&statement.discriminant),
                cases: statement
                    .cases
                    .iter()
                    .map(|case| kiln_hir::SwitchCase {
                        test: case.test.as_ref().map(|test| self.expression(test)),
                        body: self.statements(&case.consequent),
                    })
                    .collect(),
            },
            Statement::ThrowStatement(statement) => {
                HirStatementKind::Throw(self.expression(&statement.argument))
            }
            Statement::TryStatement(statement) => {
                let handler = match &statement.handler {
                    Some(clause) => {
                        let mut body: Vec<kiln_hir::Statement> = Vec::new();
                        // A catch parameter is an ordinary binding pattern, but `CatchClause`
                        // carries a single name because that is all the machine needs to put the
                        // thrown value somewhere. `catch ({ message })` therefore binds a hidden
                        // name and takes the pattern apart from it at the top of the block --
                        // the same shape an assigning `for-of` head uses, and for the same
                        // reason: one lowering of destructuring rather than two.
                        let binding = match &clause.param {
                            None => None,
                            Some(param) => match &param.pattern {
                                BindingPattern::BindingIdentifier(identifier) => {
                                    Some(identifier.name.to_string())
                                }
                                pattern => {
                                    let thrown = format!("%catch{}", statement_span.start);
                                    body.push(HirStatement {
                                        kind: HirStatementKind::Variable {
                                            // `let`, because a catch parameter is scoped to the
                                            // block and is not hoisted out of it.
                                            kind: kiln_hir::VariableKind::Let,
                                            declarations: vec![kiln_hir::VariableDeclarator {
                                                pattern: self.pattern(pattern, statement_span),
                                                initializer: Some(HirExpression {
                                                    kind: HirExpressionKind::Identifier(
                                                        thrown.clone(),
                                                    ),
                                                    span: statement_span,
                                                }),
                                                span: statement_span,
                                            }],
                                        },
                                        span: statement_span,
                                    });
                                    Some(thrown)
                                }
                            },
                        };
                        body.reserve(clause.body.body.len());
                        body.extend(
                            clause
                                .body
                                .body
                                .iter()
                                .filter_map(|statement| self.statement(statement)),
                        );
                        Some(kiln_hir::CatchClause { binding, body })
                    }
                    None => None,
                };
                HirStatementKind::Try {
                    block: self.statements(&statement.block.body),
                    handler,
                    finalizer: statement
                        .finalizer
                        .as_ref()
                        .map(|block| self.statements(&block.body))
                        .unwrap_or_default(),
                }
            }
            // `debugger` evaluates to empty in an implementation with no debugger attached,
            // which is exactly what the specification says it does. Refusing it made every test
            // that merely *contains* one unrunnable, which is a far larger claim than "no
            // debugger".
            Statement::DebuggerStatement(_) => HirStatementKind::Empty,
            // `with` is a scope whose names are not known until it runs. Lowered rather than
            // refused because refusing it made every test that merely *contains* one unrunnable,
            // and because the object environment record is a thing the compiler can express --
            // see `Compiler::with_statement`.
            Statement::WithStatement(statement) => {
                let object = self.expression(&statement.object);
                match self.statement(&statement.body) {
                    Some(body) => HirStatementKind::With {
                        object,
                        body: Box::new(body),
                    },
                    None => HirStatementKind::Empty,
                }
            }
            _ => self.unsupported_statement("statement", statement_span),
        };
        Some(HirStatement {
            kind,
            span: statement_span,
        })
    }

    fn declaration(
        &mut self,
        declaration: &Declaration<'_>,
        statement_span: HirSpan,
    ) -> Option<HirStatement> {
        let kind = match declaration {
            Declaration::VariableDeclaration(declaration) => self.variable(declaration),
            Declaration::TSTypeAliasDeclaration(_)
            | Declaration::TSInterfaceDeclaration(_)
            | Declaration::TSGlobalDeclaration(_) => return None,
            // `export function f() {}` is a declaration that also names an export. The export
            // side is read from the module record separately, so lowering only has to produce
            // the declaration -- the same one the unexported form produces.
            Declaration::FunctionDeclaration(declaration) => {
                self.function_declaration(declaration, statement_span)
            }
            Declaration::ClassDeclaration(declaration) => {
                match self.class(declaration, statement_span) {
                    Ok(class) => HirStatementKind::Class(Box::new(class)),
                    Err(feature) => self.unsupported_statement(&feature, statement_span),
                }
            }
            _ => self.unsupported_statement("TypeScript runtime declaration", statement_span),
        };
        Some(HirStatement {
            kind,
            span: statement_span,
        })
    }

    fn variable(&mut self, declaration: &VariableDeclaration<'_>) -> HirStatementKind {
        let kind = match declaration.kind {
            VariableDeclarationKind::Var => HirVariableKind::Var,
            VariableDeclarationKind::Let => HirVariableKind::Let,
            VariableDeclarationKind::Const => HirVariableKind::Const,
            // `using` and `await using` bind exactly as `const` does and add a disposal at the
            // end of the scope; the kind is what carries that through to lowering.
            VariableDeclarationKind::Using => HirVariableKind::Using,
            VariableDeclarationKind::AwaitUsing => HirVariableKind::AwaitUsing,
        };
        let declarations = declaration
            .declarations
            .iter()
            .map(|declarator| {
                let declarator_span = span(declarator.span);
                let pattern = self.pattern(&declarator.id, declarator_span);
                let mut initializer = declarator
                    .init
                    .as_ref()
                    .map(|expression| self.expression(expression));
                // `const f = () => {}` gives the function the name `f`. The specification calls
                // this named evaluation; the practical effect is that a bundle full of arrows
                // assigned to variables produces a readable stack trace instead of a column of
                // `<anonymous>`.
                if let (kiln_hir::Pattern::Identifier(name), Some(value)) =
                    (&pattern, &mut initializer)
                {
                    name_anonymous_function(value, name);
                }
                HirVariableDeclarator {
                    pattern,
                    initializer,
                    span: declarator_span,
                }
            })
            .collect();
        HirStatementKind::Variable { kind, declarations }
    }

    fn statement_or_unsupported(&mut self, statement: &Statement<'_>) -> HirStatement {
        self.statement(statement).unwrap_or_else(|| HirStatement {
            kind: self.unsupported_statement(
                "erased declaration in executable position",
                span(statement.span()),
            ),
            span: span(statement.span()),
        })
    }

    fn expression(&mut self, expression: &Expression<'_>) -> HirExpression {
        let expression_span = span(expression.span());
        // Oxc's current enum is exhaustive, but retain a defensive fallback for future AST
        // variants rather than silently treating a newly added expression as supported.
        #[allow(unreachable_patterns)]
        let kind = match expression {
            Expression::BooleanLiteral(literal) => {
                HirExpressionKind::Literal(HirLiteral::Boolean(literal.value))
            }
            Expression::NullLiteral(_) => HirExpressionKind::Literal(HirLiteral::Null),
            Expression::NumericLiteral(literal) => {
                HirExpressionKind::Literal(HirLiteral::Number(literal.value.to_bits()))
            }
            Expression::StringLiteral(literal) => {
                HirExpressionKind::Literal(HirLiteral::String(literal.value.to_string()))
            }
            Expression::Identifier(identifier) => {
                HirExpressionKind::Identifier(identifier.name.to_string())
            }
            Expression::ParenthesizedExpression(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSAsExpression(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSSatisfiesExpression(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSTypeAssertion(expression) => {
                return self.expression(&expression.expression);
            }
            Expression::TSNonNullExpression(expression) => {
                return self.expression(&expression.expression);
            }
            // `delete o.x` must not evaluate `o.x`, so it is not a unary operator over a member
            // read even though the syntax says it is.
            Expression::UnaryExpression(expression)
                if matches!(expression.operator, UnaryOperator::Delete) =>
            {
                // Parentheses do not make an operand something else: `delete (o.x)` still deletes
                // the property and `delete (x)` is still the unqualified delete the parser
                // reports as an early error in strict code.  Everything below therefore decides
                // on the operand with its parentheses taken off.
                let mut argument = &expression.argument;
                while let Expression::ParenthesizedExpression(inner) = argument {
                    argument = &inner.expression;
                }
                match argument {
                    Expression::StaticMemberExpression(member) => HirExpressionKind::Delete {
                        object: Box::new(self.expression(&member.object)),
                        property: Box::new(kiln_hir::ObjectKey::Static(
                            member.property.name.to_string(),
                        )),
                    },
                    Expression::ComputedMemberExpression(member) => HirExpressionKind::Delete {
                        object: Box::new(self.expression(&member.object)),
                        property: Box::new(kiln_hir::ObjectKey::Computed(
                            self.expression(&member.expression),
                        )),
                    },
                    Expression::PrivateFieldExpression(member) => HirExpressionKind::Delete {
                        object: Box::new(self.expression(&member.object)),
                        property: Box::new(kiln_hir::ObjectKey::Private(
                            member.field.name.to_string(),
                        )),
                    },
                    // A binding, whose answer depends on which scope declared it -- so the name
                    // travels to the IR compiler, which is the only thing that knows.
                    Expression::Identifier(identifier) => HirExpressionKind::DeleteBinding {
                        name: identifier.name.to_string(),
                    },
                    // Not a reference at all. `delete 0`, `delete f()`, `delete this`: the
                    // operand is evaluated for its effects and the answer is `true`.
                    other => HirExpressionKind::DeleteValue {
                        argument: Box::new(self.expression(other)),
                    },
                }
            }
            Expression::UnaryExpression(expression) => {
                let Some(operator) = lower_unary_operator(expression.operator) else {
                    return self.unsupported_expression(
                        &format!("unary operator `{}`", expression.operator.as_str()),
                        expression_span,
                    );
                };
                HirExpressionKind::Unary {
                    operator,
                    argument: Box::new(self.expression(&expression.argument)),
                }
            }
            Expression::BinaryExpression(expression) => {
                let Some(operator) = lower_binary_operator(expression.operator) else {
                    return self.unsupported_expression(
                        &format!("binary operator `{}`", expression.operator.as_str()),
                        expression_span,
                    );
                };
                HirExpressionKind::Binary {
                    operator,
                    left: Box::new(self.expression(&expression.left)),
                    right: Box::new(self.expression(&expression.right)),
                }
            }
            Expression::LogicalExpression(expression) => HirExpressionKind::Logical {
                operator: match expression.operator {
                    LogicalOperator::And => HirLogicalOperator::And,
                    LogicalOperator::Or => HirLogicalOperator::Or,
                    LogicalOperator::Coalesce => HirLogicalOperator::NullishCoalesce,
                },
                left: Box::new(self.expression(&expression.left)),
                right: Box::new(self.expression(&expression.right)),
            },
            Expression::AssignmentExpression(expression) => {
                let Some(operator) = lower_assignment_operator(expression.operator) else {
                    return self.unsupported_expression(
                        &format!("assignment operator `{}`", expression.operator.as_str()),
                        expression_span,
                    );
                };
                let mut value = self.expression(&expression.right);
                // `fn = function () {}` names the function `fn`, the same named evaluation a
                // declaration performs. Only the operators that can supply the binding's whole
                // value qualify: `+=` and its kin combine the function with what the binding
                // already held, so the source is not naming anything there.
                if let AssignmentTarget::AssignmentTargetIdentifier(identifier) = &expression.left
                    // `(fn) = function () {}` names nothing: the target is a parenthesized
                    // expression, and the specification asks for an identifier *reference*,
                    // which a cover grammar is not. Oxc drops the parentheses from an
                    // assignment target, so the only trace left is that the target starts
                    // after the assignment does.
                    && identifier.span.start == expression.span.start
                    && matches!(
                        operator,
                        HirAssignmentOperator::Assign
                            | HirAssignmentOperator::LogicalAnd
                            | HirAssignmentOperator::LogicalOr
                            | HirAssignmentOperator::Nullish
                    )
                {
                    name_anonymous_function(&mut value, &identifier.name);
                }
                return self.assign_to(&expression.left, operator, value, expression_span);
            }
            Expression::ConditionalExpression(expression) => HirExpressionKind::Conditional {
                test: Box::new(self.expression(&expression.test)),
                consequent: Box::new(self.expression(&expression.consequent)),
                alternate: Box::new(self.expression(&expression.alternate)),
            },
            Expression::CallExpression(expression)
                if !expression.optional && matches!(&expression.callee, Expression::Super(_)) =>
            {
                HirExpressionKind::SuperCall {
                    arguments: self.arguments(&expression.arguments),
                }
            }
            Expression::CallExpression(expression) => HirExpressionKind::Call {
                callee: Box::new(self.expression(&expression.callee)),
                arguments: self.arguments(&expression.arguments),
                optional: expression.optional,
            },
            Expression::StaticMemberExpression(member)
                if !member.optional && matches!(&member.object, Expression::Super(_)) =>
            {
                HirExpressionKind::SuperMember {
                    property: member.property.name.to_string(),
                }
            }
            Expression::StaticMemberExpression(member) => HirExpressionKind::StaticMember {
                object: Box::new(self.expression(&member.object)),
                property: member.property.name.to_string(),
                optional: member.optional,
            },
            Expression::ComputedMemberExpression(member)
                if !member.optional && matches!(&member.object, Expression::Super(_)) =>
            {
                HirExpressionKind::SuperComputedMember {
                    property: Box::new(self.expression(&member.expression)),
                }
            }
            Expression::ComputedMemberExpression(member) => HirExpressionKind::ComputedMember {
                object: Box::new(self.expression(&member.object)),
                property: Box::new(self.expression(&member.expression)),
                optional: member.optional,
            },
            Expression::PrivateFieldExpression(member) => HirExpressionKind::PrivateMember {
                object: Box::new(self.expression(&member.object)),
                name: member.field.name.to_string(),
                optional: member.optional,
            },
            Expression::CallExpression(_) => {
                return self.unsupported_expression("optional call", expression_span);
            }
            Expression::ObjectExpression(object) => {
                return self.object_literal(object, expression_span);
            }
            Expression::ArrayExpression(expression) => {
                let mut elements = Vec::with_capacity(expression.elements.len());
                for element in &expression.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            elements.push(kiln_hir::ListElement::Spread(
                                self.expression(&spread.argument),
                            ));
                        }
                        ArrayExpressionElement::Elision(_) => {
                            elements.push(kiln_hir::ListElement::Hole);
                        }
                        other => {
                            let Some(value) = other.as_expression() else {
                                return self
                                    .unsupported_expression("array element", expression_span);
                            };
                            elements.push(kiln_hir::ListElement::Item(self.expression(value)));
                        }
                    }
                }
                HirExpressionKind::Array { elements }
            }
            Expression::ArrowFunctionExpression(arrow) => {
                let Some(function) = self.arrow_function(arrow, expression_span) else {
                    return self.unsupported_expression("arrow function", expression_span);
                };
                HirExpressionKind::Function(Box::new(function))
            }
            Expression::FunctionExpression(function) => {
                let Some(function) = self.function(function, expression_span) else {
                    return self.unsupported_expression("function expression", expression_span);
                };
                HirExpressionKind::Function(Box::new(function))
            }
            Expression::ClassExpression(class) => match self.class(class, expression_span) {
                Ok(class) => HirExpressionKind::Class(Box::new(class)),
                Err(feature) => {
                    return self.unsupported_expression(&feature, expression_span);
                }
            },
            Expression::NewExpression(expression) => HirExpressionKind::New {
                callee: Box::new(self.expression(&expression.callee)),
                arguments: self.arguments(&expression.arguments),
            },
            Expression::TemplateLiteral(literal) => HirExpressionKind::Template {
                quasis: literal
                    .quasis
                    .iter()
                    .map(|quasi| {
                        // `cooked` is None only when the raw text has an invalid escape, which
                        // is a syntax error in an untagged template -- so oxc has already
                        // reported it and the empty string keeps lowering going.
                        quasi
                            .value
                            .cooked
                            .as_ref()
                            .map_or_else(String::new, |text| text.to_string())
                    })
                    .collect(),
                expressions: literal
                    .expressions
                    .iter()
                    .map(|expression| self.expression(expression))
                    .collect(),
            },
            // A tag receives the raw strings alongside the cooked ones and decides what to build
            // from them, which is a different node and not a template that happens to be called.
            Expression::TaggedTemplateExpression(tagged) => {
                // A tagged template *is* a call: the tag is evaluated as a callee, with the
                // receiver an `obj.tag` form implies, and the arguments are the template object
                // followed by each substitution. Building it as a `Call` here means none of that
                // has to be reimplemented in lowering.
                let mut arguments = Vec::with_capacity(tagged.quasi.expressions.len() + 1);
                arguments.push(ListElement::Item(HirExpression {
                    kind: HirExpressionKind::TemplateStrings {
                        cooked: tagged
                            .quasi
                            .quasis
                            .iter()
                            .map(|quasi| {
                                quasi
                                    .value
                                    .cooked
                                    .as_ref()
                                    .map(std::string::ToString::to_string)
                            })
                            .collect(),
                        raw: tagged
                            .quasi
                            .quasis
                            .iter()
                            .map(|quasi| quasi.value.raw.to_string())
                            .collect(),
                    },
                    span: expression_span,
                }));
                for substitution in &tagged.quasi.expressions {
                    arguments.push(ListElement::Item(self.expression(substitution)));
                }
                HirExpressionKind::Call {
                    callee: Box::new(self.expression(&tagged.tag)),
                    arguments,
                    optional: false,
                }
            }
            Expression::AwaitExpression(expression) => {
                HirExpressionKind::Await(Box::new(self.expression(&expression.argument)))
            }
            #[expect(unreachable_patterns, reason = "the arm above now covers this")]
            Expression::AwaitExpression(_) => {
                return self.unsupported_expression("await expression", expression_span);
            }
            Expression::YieldExpression(yielded) => HirExpressionKind::Yield {
                delegate: yielded.delegate,
                value: yielded
                    .argument
                    .as_ref()
                    .map(|value| Box::new(self.expression(value))),
            },
            Expression::UpdateExpression(update)
                if !matches!(
                    &update.argument,
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(_)
                ) =>
            {
                let (object, property) = match &update.argument {
                    SimpleAssignmentTarget::StaticMemberExpression(member) => (
                        self.expression(&member.object),
                        kiln_hir::ObjectKey::Static(member.property.name.to_string()),
                    ),
                    SimpleAssignmentTarget::ComputedMemberExpression(member) => (
                        self.expression(&member.object),
                        kiln_hir::ObjectKey::Computed(self.expression(&member.expression)),
                    ),
                    // `this.#n++`. A private name is a member target like any other here; only
                    // the key differs, and it is one the compiler already knows how to read and
                    // write. Refusing it made the ordinary way of incrementing a private counter
                    // a compile error rather than a missing optimisation.
                    SimpleAssignmentTarget::PrivateFieldExpression(member) => (
                        self.expression(&member.object),
                        kiln_hir::ObjectKey::Private(member.field.name.to_string()),
                    ),
                    _ => {
                        return self
                            .unsupported_expression("update of that target", expression_span);
                    }
                };
                HirExpressionKind::UpdateMember {
                    increment: matches!(update.operator, UpdateOperator::Increment),
                    prefix: update.prefix,
                    object: Box::new(object),
                    property: Box::new(property),
                }
            }
            Expression::UpdateExpression(update) => {
                let SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                    &update.argument
                else {
                    unreachable!("the arm above covers every other target");
                };
                HirExpressionKind::Update {
                    increment: matches!(update.operator, UpdateOperator::Increment),
                    prefix: update.prefix,
                    target: identifier.name.to_string(),
                }
            }
            Expression::ThisExpression(_) => HirExpressionKind::This,
            Expression::SequenceExpression(sequence) => HirExpressionKind::Sequence {
                operands: sequence
                    .expressions
                    .iter()
                    .map(|operand| self.expression(operand))
                    .collect(),
            },
            Expression::BigIntLiteral(literal) => {
                // The raw text carries the base prefix and the `n` suffix, and may carry numeric
                // separators. Normalising to plain decimal digits here means nothing downstream
                // has to know how the source spelled it.
                match literal
                    .raw
                    .as_ref()
                    .and_then(|raw| bigint_digits(raw.as_str()))
                {
                    Some(digits) => HirExpressionKind::Literal(HirLiteral::BigInt(digits)),
                    None => return self.unsupported_expression("bigint literal", expression_span),
                }
            }
            Expression::RegExpLiteral(literal) => HirExpressionKind::RegExp {
                // The source text, not a compiled program: `re.source` hands the pattern back to
                // the program, and compiling belongs to whoever owns the engine.
                pattern: literal.regex.pattern.text.to_string(),
                flags: literal.regex.flags.to_string(),
            },
            Expression::Super(_) => {
                return self.unsupported_expression("super expression", expression_span);
            }
            // The chain is what the `?.` jumps leave, so the boundary is marked here rather
            // than inferred later from where the optional links happen to sit.
            Expression::ChainExpression(chain) => {
                let inner = match &chain.expression {
                    oxc_ast::ast::ChainElement::CallExpression(call) => HirExpression {
                        kind: HirExpressionKind::Call {
                            callee: Box::new(self.expression(&call.callee)),
                            arguments: self.arguments(&call.arguments),
                            optional: call.optional,
                        },
                        span: expression_span,
                    },
                    other => match other.as_member_expression() {
                        Some(member) => self.member_expression(member, expression_span),
                        None => self.unsupported_expression("chain element", expression_span),
                    },
                };
                // A chain around something already refused adds nothing, and wrapping it would
                // hide which construct was refused from anything matching on the kind.
                if matches!(inner.kind, HirExpressionKind::Unsupported { .. }) {
                    return inner;
                }
                HirExpressionKind::Chain {
                    expression: Box::new(inner),
                }
            }
            Expression::ImportExpression(import) => {
                // `import.defer(…)` and `import.source(…)` are not `import(…)` with a flag: each
                // hands back a different thing -- a deferred namespace, a module source object --
                // and lowering one as an ordinary import would resolve its promise with the wrong
                // value rather than saying it is not implemented.
                if import.phase.is_some() {
                    return self.unsupported_expression(
                        "phased dynamic import expression",
                        expression_span,
                    );
                }
                HirExpressionKind::ImportCall {
                    specifier: Box::new(self.expression(&import.source)),
                    options: import
                        .options
                        .as_ref()
                        .map(|options| Box::new(self.expression(options))),
                }
            }
            // `#x in o` is the brand check: it asks whether `o` carries this class's private
            // field, which is exactly what reading one already tests for.
            Expression::PrivateInExpression(expression) => HirExpressionKind::PrivateIn {
                name: expression.left.name.to_string(),
                object: Box::new(self.expression(&expression.right)),
            },
            Expression::PrivateInExpression(_) => {
                return self.unsupported_expression("private-in expression", expression_span);
            }
            Expression::ImportMeta(_) => {
                let (url, dirname, filename) = if let Some(p) = self.path {
                    let path_buf = Path::new(p);
                    let abs_path = if path_buf.is_absolute() {
                        path_buf.to_path_buf()
                    } else if let Ok(cwd) = std::env::current_dir() {
                        cwd.join(path_buf)
                    } else {
                        path_buf.to_path_buf()
                    };
                    let filename_str = abs_path.to_string_lossy().into_owned();
                    let dirname_str = abs_path
                        .parent()
                        .map(|parent| parent.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let url_str = format!("file://{filename_str}");
                    (url_str, dirname_str, filename_str)
                } else {
                    (
                        "file:///unknown".to_string(),
                        String::new(),
                        "unknown".to_string(),
                    )
                };

                HirExpressionKind::Object {
                    members: vec![
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("url".to_string()),
                            value: HirExpression {
                                kind: HirExpressionKind::Literal(HirLiteral::String(url)),
                                span: expression_span,
                            },
                            form: kiln_hir::PropertyForm::KeyValue,
                            span: expression_span,
                        }),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("dirname".to_string()),
                            value: HirExpression {
                                kind: HirExpressionKind::Literal(HirLiteral::String(dirname)),
                                span: expression_span,
                            },
                            form: kiln_hir::PropertyForm::KeyValue,
                            span: expression_span,
                        }),
                        kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                            key: kiln_hir::ObjectKey::Static("filename".to_string()),
                            value: HirExpression {
                                kind: HirExpressionKind::Literal(HirLiteral::String(filename)),
                                span: expression_span,
                            },
                            form: kiln_hir::PropertyForm::KeyValue,
                            span: expression_span,
                        }),
                    ],
                }
            }
            Expression::NewTarget(_) => HirExpressionKind::NewTarget,
            Expression::JSXElement(_) | Expression::JSXFragment(_) => {
                return self.unsupported_expression("JSX expression", expression_span);
            }
            Expression::TSInstantiationExpression(_) => {
                return self.unsupported_expression(
                    "TypeScript instantiation expression",
                    expression_span,
                );
            }
            Expression::V8IntrinsicExpression(_) => {
                return self.unsupported_expression("V8 intrinsic expression", expression_span);
            }
            _ => return self.unsupported_expression("expression", expression_span),
        };
        HirExpression {
            kind,
            span: expression_span,
        }
    }

    /// Lower a function declaration, or refuse it as a whole.
    ///
    /// Refusing the declaration outright rather than lowering a partial function is deliberate:
    /// a generator whose `yield`s were dropped, or an async function whose `await`s were, would
    /// be a function that runs and produces a confidently wrong answer. A missing function is a
    /// diagnostic; a silently altered one is a bug report six months later.
    /// A `export default function`/`class` whose lowering may have been refused.
    fn default_declaration(
        &mut self,
        lowered: Result<HirStatementKind, String>,
        statement_span: HirSpan,
    ) -> HirStatementKind {
        match lowered {
            Ok(kind) => kind,
            Err(feature) => self.unsupported_statement(&feature, statement_span),
        }
    }

    /// An assignment target read as a binding pattern.
    ///
    /// `None` when the target holds something a [`kiln_hir::Pattern`] cannot say. What remains
    /// in that bucket is narrow -- an optional chain in target position, which is not a valid
    /// assignment target anyway -- because a pattern leaf may now be a reference as well as a
    /// name.
    /// Store `value` into whatever `target` names.
    ///
    /// The left-hand side of an assignment and the head of an assigning `for-of` or `for-in` are
    /// the same grammar, so both reach the rules here. A second copy for the loops would drift:
    /// every target shape below took its own reasoning to get right, and a shape added to one
    /// copy would quietly go missing from the other.
    ///
    /// `value` arrives already lowered because the loops' value is a binding rather than an
    /// expression the source contains.
    fn assign_to(
        &mut self,
        target: &AssignmentTarget<'_>,
        operator: HirAssignmentOperator,
        value: HirExpression,
        span: HirSpan,
    ) -> HirExpression {
        let kind = match target {
            AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                HirExpressionKind::Assignment {
                    operator,
                    target: identifier.name.to_string(),
                    value: Box::new(value),
                }
            }
            AssignmentTarget::ComputedMemberExpression(member)
                if !member.optional && matches!(member.object, Expression::Super(_)) =>
            {
                HirExpressionKind::SuperComputedAssignment {
                    operator,
                    property: Box::new(self.expression(&member.expression)),
                    value: Box::new(value),
                }
            }
            AssignmentTarget::ComputedMemberExpression(member) if !member.optional => {
                HirExpressionKind::ComputedAssignment {
                    operator,
                    object: Box::new(self.expression(&member.object)),
                    property: Box::new(self.expression(&member.expression)),
                    value: Box::new(value),
                }
            }
            AssignmentTarget::StaticMemberExpression(member)
                if !member.optional && matches!(member.object, Expression::Super(_)) =>
            {
                HirExpressionKind::SuperMemberAssignment {
                    operator,
                    property: member.property.name.to_string(),
                    value: Box::new(value),
                }
            }
            AssignmentTarget::StaticMemberExpression(member) if !member.optional => {
                HirExpressionKind::MemberAssignment {
                    operator,
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string(),
                    value: Box::new(value),
                }
            }
            AssignmentTarget::PrivateFieldExpression(member) => {
                HirExpressionKind::PrivateAssignment {
                    operator,
                    object: Box::new(self.expression(&member.object)),
                    name: member.field.name.to_string(),
                    value: Box::new(value),
                }
            }
            // `[a, b] = pair` and `({ a } = record)`. Only `=` takes a pattern, so a compound
            // operator here is a different error than an unsupported shape.
            AssignmentTarget::ArrayAssignmentTarget(_)
            | AssignmentTarget::ObjectAssignmentTarget(_)
                if operator == HirAssignmentOperator::Assign =>
            {
                match self.assignment_pattern(target) {
                    Some(pattern) => HirExpressionKind::PatternAssignment {
                        pattern,
                        value: Box::new(value),
                    },
                    None => {
                        return self
                            .unsupported_expression("destructuring assignment target", span);
                    }
                }
            }
            _ => {
                return self.unsupported_expression("non-identifier assignment", span);
            }
        };
        HirExpression { kind, span }
    }

    fn assignment_pattern(&mut self, target: &AssignmentTarget<'_>) -> Option<kiln_hir::Pattern> {
        match target {
            AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                Some(kiln_hir::Pattern::Identifier(identifier.name.to_string()))
            }
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                let mut elements = Vec::with_capacity(array.elements.len());
                for element in &array.elements {
                    elements.push(match element {
                        // A hole assigns nothing, and skips the index all the same.
                        None => None,
                        Some(element) => Some(self.assignment_maybe_default(element)?),
                    });
                }
                let rest = match &array.rest {
                    Some(rest) => Some(Box::new(self.assignment_pattern(&rest.target)?)),
                    None => None,
                };
                Some(kiln_hir::Pattern::Array { elements, rest })
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                let mut properties = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    match property {
                        oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                            shorthand,
                        ) => {
                            let name = shorthand.binding.name.to_string();
                            let value = kiln_hir::Pattern::Identifier(name.clone());
                            // `({ a = 1 } = o)`: the default belongs to the value, exactly
                            // as it does in a declaration.
                            let value = match &shorthand.init {
                                Some(default) => {
                                    let mut default = self.expression(default);
                                    name_anonymous_default(&value, &mut default);
                                    kiln_hir::Pattern::WithDefault {
                                        inner: Box::new(value),
                                        default: Box::new(default),
                                    }
                                }
                                None => value,
                            };
                            properties.push(kiln_hir::PatternProperty {
                                key: kiln_hir::PatternKey::Static(name),
                                value,
                            });
                        }
                        oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                            named,
                        ) => {
                            let key = if named.computed {
                                kiln_hir::PatternKey::Computed(
                                    self.expression(named.name.as_expression()?),
                                )
                            } else {
                                kiln_hir::PatternKey::Static(named.name.static_name()?.to_string())
                            };
                            properties.push(kiln_hir::PatternProperty {
                                key,
                                value: self.assignment_maybe_default(&named.binding)?,
                            });
                        }
                    }
                }
                let rest = match &object.rest {
                    Some(rest) => Some(Box::new(self.assignment_pattern(&rest.target)?)),
                    None => None,
                };
                Some(kiln_hir::Pattern::Object { properties, rest })
            }
            other => self.reference_pattern(other),
        }
    }

    /// A target that stores through a property reference rather than binding a name.
    ///
    /// The three shapes are the ones [`Lowerer::assign_to`] already distinguishes for a plain
    /// `=`, and for the same reason: a computed key is an expression to evaluate, a private name
    /// has to already exist on the object, and a static name is neither.
    fn reference_pattern(&mut self, target: &AssignmentTarget<'_>) -> Option<kiln_hir::Pattern> {
        match target {
            AssignmentTarget::ComputedMemberExpression(member)
                if !member.optional && matches!(member.object, Expression::Super(_)) =>
            {
                Some(kiln_hir::Pattern::Computed {
                    object: Box::new(HirExpression {
                        kind: HirExpressionKind::This,
                        span: span(member.span),
                    }),
                    property: Box::new(self.expression(&member.expression)),
                })
            }
            AssignmentTarget::ComputedMemberExpression(member) if !member.optional => {
                Some(kiln_hir::Pattern::Computed {
                    object: Box::new(self.expression(&member.object)),
                    property: Box::new(self.expression(&member.expression)),
                })
            }
            // `[super.x] = xs` takes the same shortcut as `super.x = v`, and inherits the same
            // single wrong case; the reasoning is written out in `assign_to`.
            AssignmentTarget::StaticMemberExpression(member)
                if !member.optional && matches!(member.object, Expression::Super(_)) =>
            {
                Some(kiln_hir::Pattern::Member {
                    object: Box::new(HirExpression {
                        kind: HirExpressionKind::This,
                        span: span(member.span),
                    }),
                    property: member.property.name.to_string(),
                })
            }
            AssignmentTarget::StaticMemberExpression(member) if !member.optional => {
                Some(kiln_hir::Pattern::Member {
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string(),
                })
            }
            AssignmentTarget::PrivateFieldExpression(member) => {
                Some(kiln_hir::Pattern::PrivateMember {
                    object: Box::new(self.expression(&member.object)),
                    name: member.field.name.to_string(),
                })
            }
            _ => None,
        }
    }

    /// One element of a destructuring assignment, which may carry a default.
    fn assignment_maybe_default(
        &mut self,
        element: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
    ) -> Option<kiln_hir::Pattern> {
        match element {
            oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with) => {
                let inner = self.assignment_pattern(&with.binding)?;
                let mut default = self.expression(&with.init);
                // `[(fn) = function () {}] = []` names nothing, for the reason a parenthesized
                // assignment target does not: the cover grammar is not an identifier reference.
                // Oxc drops the parentheses here too, so what is left of them is that the
                // target starts after the element does.
                if with.binding.span().start == with.span.start {
                    name_anonymous_default(&inner, &mut default);
                }
                Some(kiln_hir::Pattern::WithDefault {
                    inner: Box::new(inner),
                    default: Box::new(default),
                })
            }
            other => self.assignment_pattern(other.as_assignment_target()?),
        }
    }

    fn function_declaration(
        &mut self,
        declaration: &Function<'_>,
        statement_span: HirSpan,
    ) -> HirStatementKind {
        let Some(function) = self.function(declaration, statement_span) else {
            return HirStatementKind::Unsupported {
                feature: "function declaration".to_string(),
            };
        };
        HirStatementKind::Function(Box::new(function))
    }

    /// A `for` initialiser is either a declaration or an expression.
    fn for_init(
        &mut self,
        init: &ForStatementInit<'_>,
        statement_span: HirSpan,
    ) -> Option<HirStatement> {
        match init {
            ForStatementInit::VariableDeclaration(declaration) => Some(HirStatement {
                kind: self.variable(declaration),
                span: span(declaration.span),
            }),
            other => {
                let expression = other.as_expression()?;
                Some(HirStatement {
                    kind: HirStatementKind::Expression(self.expression(expression)),
                    span: statement_span,
                })
            }
        }
    }

    /// Lower a binding pattern.
    ///
    /// An unsupported shape becomes an identifier binding a name no source can produce, so the
    /// diagnostic already recorded is the failure the user sees rather than a second, confusing
    /// one about an unresolved name.
    fn pattern(&mut self, pattern: &BindingPattern<'_>, span: HirSpan) -> kiln_hir::Pattern {
        match pattern {
            BindingPattern::BindingIdentifier(identifier) => {
                kiln_hir::Pattern::Identifier(identifier.name.to_string())
            }
            BindingPattern::AssignmentPattern(assignment) => {
                let inner = self.pattern(&assignment.left, span);
                let mut default = self.expression(&assignment.right);
                name_anonymous_default(&inner, &mut default);
                kiln_hir::Pattern::WithDefault {
                    inner: Box::new(inner),
                    default: Box::new(default),
                }
            }
            BindingPattern::ObjectPattern(object) => {
                let mut properties = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    let key = if property.computed {
                        let Some(key) = property.key.as_expression() else {
                            self.unsupported("computed key in a destructuring pattern", span);
                            return kiln_hir::Pattern::Identifier(String::new());
                        };
                        kiln_hir::PatternKey::Computed(self.expression(key))
                    } else {
                        let Some(key) = property.key.static_name() else {
                            self.unsupported("destructuring key", span);
                            return kiln_hir::Pattern::Identifier(String::new());
                        };
                        kiln_hir::PatternKey::Static(key.to_string())
                    };
                    properties.push(kiln_hir::PatternProperty {
                        key,
                        value: self.pattern(&property.value, span),
                    });
                }
                let rest = match &object.rest {
                    None => None,
                    Some(rest) => match &rest.argument {
                        BindingPattern::BindingIdentifier(identifier) => Some(Box::new(
                            kiln_hir::Pattern::Identifier(identifier.name.to_string()),
                        )),
                        // `{ ...{ a } }` is not valid as a binding rest, and a declaration can
                        // never store through a reference the way an assignment can.
                        _ => {
                            self.unsupported("destructuring rest pattern", span);
                            return kiln_hir::Pattern::Identifier(String::new());
                        }
                    },
                };
                kiln_hir::Pattern::Object { properties, rest }
            }
            BindingPattern::ArrayPattern(array) => kiln_hir::Pattern::Array {
                elements: array
                    .elements
                    .iter()
                    .map(|element| element.as_ref().map(|element| self.pattern(element, span)))
                    .collect(),
                rest: array
                    .rest
                    .as_ref()
                    .map(|rest| Box::new(self.pattern(&rest.argument, span))),
            },
        }
    }

    /// Lower a class, or refuse it whole, naming the member that stopped it.
    ///
    /// Refusing wholesale for the same reason a partial function is refused: a class missing a
    /// method it declared would construct successfully and fail somewhere else entirely.
    ///
    /// The refusal carries the specific feature rather than a bare `None`, because the callers
    /// used to substitute "class expression" for every cause: a static block, an async method
    /// and a computed key all arrived at the guard corpus as one indistinguishable bucket, and
    /// the backlog could not be read out of it. The name that reaches the diagnostic has to be
    /// the one that describes the member, not the one that describes the syntactic position.
    /// The key a class member declares, including the private form.
    ///
    /// A private name is not a string key: `#x` and `"#x"` name different properties, and only
    /// the first is hidden from everything that enumerates.
    fn member_key(
        &mut self,
        key: &oxc_ast::ast::PropertyKey<'_>,
        computed: bool,
    ) -> Option<kiln_hir::ObjectKey> {
        if let oxc_ast::ast::PropertyKey::PrivateIdentifier(private) = key {
            return Some(kiln_hir::ObjectKey::Private(private.name.to_string()));
        }
        if computed {
            return Some(kiln_hir::ObjectKey::Computed(
                self.expression(key.as_expression()?),
            ));
        }
        Some(kiln_hir::ObjectKey::Static(key.static_name()?.to_string()))
    }

    fn class(
        &mut self,
        class: &Class<'_>,
        statement_span: HirSpan,
    ) -> Result<kiln_hir::Class, String> {
        let mut constructor = None;
        let mut methods = Vec::new();
        let mut initialisers = Vec::new();

        for element in &class.body.body {
            let method = match element {
                ClassElement::PropertyDefinition(property) => {
                    let key = self
                        .member_key(&property.key, property.computed)
                        .ok_or("class field name")?;
                    let mut value = property.value.as_ref().map(|value| self.expression(value));
                    // `class C { handle = () => {} }` names the function `handle`, for the same
                    // reason an object property does. A private field's name keeps its `#`,
                    // which is what the field is called; a computed key names nothing here,
                    // because the name is whatever the key evaluates to at construction time.
                    if let Some(value) = &mut value {
                        match &key {
                            kiln_hir::ObjectKey::Static(name) => {
                                name_anonymous_function(value, name);
                            }
                            kiln_hir::ObjectKey::Private(name) => {
                                name_anonymous_function(value, &format!("#{name}"));
                            }
                            kiln_hir::ObjectKey::Computed(_) => {}
                        }
                    }
                    initialisers.push(kiln_hir::ClassInitialiser::Field(kiln_hir::ClassField {
                        key,
                        value,
                        is_static: property.r#static,
                    }));
                    continue;
                }
                ClassElement::MethodDefinition(method) => method,
                ClassElement::StaticBlock(block) => {
                    initialisers.push(kiln_hir::ClassInitialiser::StaticBlock(HirFunction {
                        // Anonymous, and it binds no name: nothing can refer to a static block,
                        // so a name here would only show up in a stack frame that should read
                        // as the class body it is.
                        name: None,
                        binds_own_name: false,
                        arrow: false,
                        generator: false,
                        asynchronous: false,
                        parameters: Vec::new(),
                        rest: None,
                        return_type: None,
                        body: self.statements(&block.body),
                        // A class body is strict whatever encloses it, and the block is part of
                        // the body.
                        strict: true,
                        span: span(block.span),
                    }));
                    continue;
                }
                // `accessor x = v` is three members written as one: a backing slot, and the
                // pair of functions that read and write it. Lowered here rather than in the IR
                // because that is all it is -- a private field plus `get x` / `set x` -- and
                // putting the getter and setter among the class's other methods, at the
                // position the source wrote them, is what makes a getter or setter written
                // beside it override exactly the half it declares.
                ClassElement::AccessorProperty(accessor) => {
                    // A computed key would have to be evaluated *once* and used by all three
                    // members. Two of them are separate `ClassMethod`s here, so lowering one
                    // would evaluate the key twice -- an observable wrong answer, where a
                    // refusal is only a missing feature.
                    if accessor.computed {
                        return Err("computed class accessor property".into());
                    }
                    // `accessor #x` needs a private getter/setter pair under `#x` beside a
                    // second private name for the storage, and the duplicate-name rules that
                    // go with it. Not attempted.
                    if matches!(
                        accessor.key,
                        oxc_ast::ast::PropertyKey::PrivateIdentifier(_)
                    ) {
                        return Err("private class accessor property".into());
                    }
                    let Some(key) = accessor.key.static_name() else {
                        return Err("class accessor property name".into());
                    };
                    let key = key.to_string();
                    // The backing slot is a private name the program cannot have written: `%`
                    // is not an identifier character, so this can never collide with a `#x` the
                    // class declares, and nothing can reach the storage except the two
                    // functions below.
                    let storage = format!("%accessor:{key}");
                    let member_span = span(accessor.span);
                    let this = || HirExpression {
                        kind: HirExpressionKind::This,
                        span: member_span,
                    };
                    let read = HirExpression {
                        kind: HirExpressionKind::PrivateMember {
                            object: Box::new(this()),
                            name: storage.clone(),
                            optional: false,
                        },
                        span: member_span,
                    };
                    // The parameter is named where no source can name it, so the body cannot be
                    // shadowed by anything and reads exactly the value it was handed.
                    let written = HirExpression {
                        kind: HirExpressionKind::PrivateAssignment {
                            operator: kiln_hir::AssignmentOperator::Assign,
                            object: Box::new(this()),
                            name: storage.clone(),
                            value: Box::new(HirExpression {
                                kind: HirExpressionKind::Identifier("%value".to_owned()),
                                span: member_span,
                            }),
                        },
                        span: member_span,
                    };
                    let accessor_function =
                        |parameters: Vec<kiln_hir::Parameter>, body: Vec<HirStatement>| {
                            HirFunction {
                                name: None,
                                binds_own_name: false,
                                arrow: false,
                                generator: false,
                                asynchronous: false,
                                parameters,
                                rest: None,
                                return_type: None,
                                body,
                                // A class body is strict whatever encloses it.
                                strict: true,
                                span: member_span,
                            }
                        };
                    let mut value = accessor.value.as_ref().map(|value| self.expression(value));
                    if let Some(value) = &mut value {
                        name_anonymous_function(value, &key);
                    }
                    initialisers.push(kiln_hir::ClassInitialiser::Field(kiln_hir::ClassField {
                        key: kiln_hir::ObjectKey::Private(storage),
                        value,
                        is_static: accessor.r#static,
                    }));
                    methods.push(kiln_hir::ClassMethod {
                        key: kiln_hir::ObjectKey::Static(key.clone()),
                        function: accessor_function(
                            Vec::new(),
                            vec![HirStatement {
                                kind: HirStatementKind::Return(Some(read)),
                                span: member_span,
                            }],
                        ),
                        is_static: accessor.r#static,
                        kind: kiln_hir::MethodKind::Get,
                    });
                    methods.push(kiln_hir::ClassMethod {
                        key: kiln_hir::ObjectKey::Static(key),
                        function: accessor_function(
                            vec![kiln_hir::Parameter {
                                pattern: kiln_hir::Pattern::Identifier("%value".to_owned()),
                                type_annotation: None,
                                span: member_span,
                            }],
                            vec![HirStatement {
                                kind: HirStatementKind::Expression(written),
                                span: member_span,
                            }],
                        ),
                        is_static: accessor.r#static,
                        kind: kiln_hir::MethodKind::Set,
                    });
                    continue;
                }
                ClassElement::TSIndexSignature(_) => {
                    return Err("TypeScript index signature".into());
                }
            };
            let lowered = self
                .function(
                    &method.value,
                    self.method_definition_span(span(method.span), method.r#static),
                )
                .ok_or("class method without a body")?;
            match method.kind {
                MethodDefinitionKind::Constructor => constructor = Some(lowered),
                kind @ (MethodDefinitionKind::Method
                | MethodDefinitionKind::Get
                | MethodDefinitionKind::Set) => {
                    let key = self
                        .member_key(&method.key, method.computed)
                        .ok_or("class method name")?;
                    methods.push(kiln_hir::ClassMethod {
                        key,
                        function: lowered,
                        is_static: method.r#static,
                        kind: match kind {
                            MethodDefinitionKind::Get => kiln_hir::MethodKind::Get,
                            MethodDefinitionKind::Set => kiln_hir::MethodKind::Set,
                            _ => kiln_hir::MethodKind::Normal,
                        },
                    });
                }
            }
        }

        Ok(kiln_hir::Class {
            name: class.id.as_ref().map(|id| id.name.to_string()),
            // Supplied by the named evaluation that encloses this, if any: the parser sees only
            // that the class is anonymous.
            inferred_name: None,
            superclass: class
                .heritage
                .as_ref()
                .map(|heritage| self.expression(&heritage.expression)),
            constructor,
            methods,
            initialisers,
            span: statement_span,
        })
    }

    /// Lower a parameter list, including defaults and a rest parameter.
    ///
    /// oxc keeps a parameter's default on the `FormalParameter` rather than inside its binding
    /// pattern, so reading only `item.pattern` sees a plain identifier for `f(x = 1)` and drops
    /// the default without a word. That is exactly the silent-alteration this frontend refuses
    /// everywhere else, and it is why the default is folded in here rather than in `pattern`.
    fn parameters(
        &mut self,
        parameters: &FormalParameters<'_>,
    ) -> (Vec<HirParameter>, Option<kiln_hir::Pattern>) {
        let lowered = parameters
            .items
            .iter()
            .map(|item| {
                let item_span = span(item.span);
                let pattern = self.pattern(&item.pattern, item_span);
                let type_annotation = item
                    .type_annotation
                    .as_ref()
                    .and_then(|t| lower_type_annotation(t));
                HirParameter {
                    pattern: match &item.initializer {
                        Some(default) => {
                            let mut default = self.expression(default);
                            name_anonymous_default(&pattern, &mut default);
                            kiln_hir::Pattern::WithDefault {
                                inner: Box::new(pattern),
                                default: Box::new(default),
                            }
                        }
                        None => pattern,
                    },
                    type_annotation,
                    span: item_span,
                }
            })
            .collect();
        let rest = parameters
            .rest
            .as_ref()
            .map(|rest| self.pattern(&rest.rest.argument, span(rest.span)));
        (lowered, rest)
    }

    /// Lower an arrow function.
    ///
    /// An expression body becomes a single `return`, which is what it means. The arrow flag is
    /// carried even though neither `this` nor `arguments` is implemented: the difference is real,
    /// and a later step that has to reconstruct it from the shape of the body would get it wrong.
    fn arrow_function(
        &mut self,
        arrow: &ArrowFunctionExpression<'_>,
        expression_span: HirSpan,
    ) -> Option<HirFunction> {
        let (parameters, rest) = self.parameters(&arrow.params);
        let return_type = arrow
            .return_type
            .as_ref()
            .and_then(|t| lower_type_annotation(t));

        let body = match &arrow.body {
            ArrowFunctionBody::FunctionBody(block) => self.statements(&block.statements),
            // A concise body (`x => x + 1`) is an expression whose value is returned, not an
            // expression statement whose value is discarded.
            concise => {
                let Some(expression) = concise.as_expression() else {
                    self.unsupported("arrow function body", expression_span);
                    return None;
                };
                vec![HirStatement {
                    kind: HirStatementKind::Return(Some(self.expression(expression))),
                    span: expression_span,
                }]
            }
        };

        Some(HirFunction {
            name: None,
            // An arrow can never be given a name, so it never binds one.
            binds_own_name: false,
            arrow: true,
            generator: false,
            asynchronous: arrow.r#async,
            parameters,
            rest,
            return_type,
            body,
            // A concise body has no prologue to open with, so only the braced form can say so.
            strict: match &arrow.body {
                ArrowFunctionBody::FunctionBody(block) => opens_strict(&block.directives),
                _ => false,
            },
            span: expression_span,
        })
    }

    /// The shared part of every function form. `None` means a diagnostic was already recorded.
    fn function(
        &mut self,
        declaration: &Function<'_>,
        statement_span: HirSpan,
    ) -> Option<HirFunction> {
        let (parameters, rest) = self.parameters(&declaration.params);
        let return_type = declaration
            .return_type
            .as_ref()
            .and_then(|t| lower_type_annotation(t));

        let body = declaration.body.as_ref()?;
        let statements = self.statements(&body.statements);

        Some(HirFunction {
            name: declaration.id.as_ref().map(|id| id.name.to_string()),
            binds_own_name: declaration.id.is_some(),
            arrow: false,
            generator: declaration.generator,
            asynchronous: declaration.r#async,
            parameters,
            rest,
            return_type,
            body: statements,
            strict: opens_strict(&body.directives),
            span: statement_span,
        })
    }

    /// The body of a `for-of` or `for-in` whose head assigns rather than binds: assign the
    /// step's value across, then run what the source wrote.
    ///
    /// The source's body stays one statement rather than being spliced in, so a `let` it
    /// declares that shadows the target does not put the assignment in that binding's temporal
    /// dead zone -- `for (x of xs) { let x; }` assigns the outer `x`, as it does with no
    /// desugaring at all.
    fn assigning_loop_body(
        &mut self,
        assignment: HirExpression,
        body: &Statement<'_>,
        span: HirSpan,
    ) -> Box<HirStatement> {
        let assignment = HirStatement {
            span: assignment.span,
            kind: HirStatementKind::Expression(assignment),
        };
        let body = self.statement_or_unsupported(body);
        Box::new(HirStatement {
            kind: HirStatementKind::Block(vec![assignment, body]),
            span,
        })
    }

    /// The `kind` and single bound name of a `for-in` or `for-of` head, when it is a simple
    /// declaration of one identifier. `None` for every other shape, each of which needs its own
    /// lowering and gets its own diagnostic at the call site.
    fn for_binding(&mut self, left: &ForStatementLeft<'_>) -> Option<(HirVariableKind, String)> {
        let ForStatementLeft::VariableDeclaration(declaration) = left else {
            return None;
        };
        let declarator = declaration.declarations.first()?;
        let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
            return None;
        };
        let kind = match declaration.kind {
            VariableDeclarationKind::Var => HirVariableKind::Var,
            VariableDeclarationKind::Const => HirVariableKind::Const,
            _ => HirVariableKind::Let,
        };
        Some((kind, identifier.name.to_string()))
    }

    /// Lower a member access, optional or not. Shared with the chain path, where the same three
    /// shapes appear behind a `?.`.
    fn member_expression(
        &mut self,
        member: &oxc_ast::ast::MemberExpression<'_>,
        expression_span: HirSpan,
    ) -> HirExpression {
        let kind = match member {
            oxc_ast::ast::MemberExpression::StaticMemberExpression(member) => {
                HirExpressionKind::StaticMember {
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string(),
                    optional: member.optional,
                }
            }
            oxc_ast::ast::MemberExpression::ComputedMemberExpression(member) => {
                HirExpressionKind::ComputedMember {
                    object: Box::new(self.expression(&member.object)),
                    property: Box::new(self.expression(&member.expression)),
                    optional: member.optional,
                }
            }
            oxc_ast::ast::MemberExpression::PrivateFieldExpression(member) => {
                HirExpressionKind::PrivateMember {
                    object: Box::new(self.expression(&member.object)),
                    name: member.field.name.to_string(),
                    optional: member.optional,
                }
            }
        };
        HirExpression {
            kind,
            span: expression_span,
        }
    }

    /// Lower an argument list, where a spread contributes however many an iterable holds.
    fn arguments(&mut self, arguments: &oxc_allocator::Vec<'_, Argument<'_>>) -> Vec<ListElement> {
        let mut lowered = Vec::with_capacity(arguments.len());
        for argument in arguments {
            lowered.push(match argument {
                Argument::SpreadElement(spread) => {
                    ListElement::Spread(self.expression(&spread.argument))
                }
                other => match other.as_expression() {
                    Some(expression) => ListElement::Item(self.expression(expression)),
                    // Every non-spread argument is an expression; this arm exists because the
                    // oxc enum cannot say so in its type.
                    None => ListElement::Item(
                        self.unsupported_expression("argument", HirSpan::default()),
                    ),
                },
            });
        }
        lowered
    }

    /// Lower an object literal, or refuse the whole literal.
    ///
    /// Refusing wholesale rather than dropping the members it cannot lower: an object missing
    /// some of its properties looks like a valid object and would be wrong everywhere it was
    /// used, whereas a missing object is a diagnostic at the point of the mistake.
    fn object_literal(
        &mut self,
        expression: &ObjectExpression<'_>,
        expression_span: HirSpan,
    ) -> HirExpression {
        let mut members = Vec::with_capacity(expression.properties.len());
        for property in &expression.properties {
            let property = match property {
                ObjectPropertyKind::ObjectProperty(property) => property,
                ObjectPropertyKind::SpreadProperty(spread) => {
                    members.push(kiln_hir::ObjectMember::Spread(
                        self.expression(&spread.argument),
                    ));
                    continue;
                }
            };
            // A shorthand method is a property whose value is a function expression. It differs
            // from one written longhand only in having no `prototype` and not being constructable,
            // neither of which is observable yet. A getter or setter is not: reading it is a call.
            if !matches!(property.kind, PropertyKind::Init) {
                let key = if property.computed {
                    match property.key.as_expression() {
                        Some(key) => kiln_hir::ObjectKey::Computed(self.expression(key)),
                        None => {
                            return self
                                .unsupported_expression("computed object key", expression_span);
                        }
                    }
                } else {
                    match property.key.static_name() {
                        Some(key) => kiln_hir::ObjectKey::Static(key.to_string()),
                        None => return self.unsupported_expression("object key", expression_span),
                    }
                };
                let Expression::FunctionExpression(function) = &property.value else {
                    return self.unsupported_expression("accessor body", expression_span);
                };
                // The property's span, not the object's: `Function.prototype.toString` on an
                // accessor answers `"get p() {}"` -- the keyword and the key are part of the
                // definition, and the object literal around it is not.
                let Some(function) = self.function(function, span(property.span)) else {
                    return self.unsupported_expression("accessor body", expression_span);
                };
                members.push(kiln_hir::ObjectMember::Accessor {
                    key,
                    kind: if matches!(property.kind, PropertyKind::Get) {
                        kiln_hir::MethodKind::Get
                    } else {
                        kiln_hir::MethodKind::Set
                    },
                    function,
                });
                continue;
            }
            let key = if property.computed {
                let Some(key) = property.key.as_expression() else {
                    return self.unsupported_expression("computed object key", expression_span);
                };
                kiln_hir::ObjectKey::Computed(self.expression(key))
            } else {
                let Some(key) = property.key.static_name() else {
                    return self.unsupported_expression("object key", expression_span);
                };
                kiln_hir::ObjectKey::Static(key.to_string())
            };
            // Only `{ __proto__: v }` sets a prototype, so which of the three productions wrote
            // this pair has to survive lowering. Oxc reports the shorthand and the method
            // separately, and a pair is neither only when it was written longhand.
            let form = if property.shorthand {
                kiln_hir::PropertyForm::Shorthand
            } else if property.method {
                kiln_hir::PropertyForm::Method
            } else {
                kiln_hir::PropertyForm::KeyValue
            };
            let mut value = self.expression(&property.value);
            // A shorthand method's definition starts at its key: `({ m(x) {} }).m.toString()` is
            // `"m(x) {}"`. Oxc spans the function expression alone, which starts at the
            // parenthesis, so the property's span is the one that matches what was written.
            if property.method
                && let HirExpressionKind::Function(function) = &mut value.kind
            {
                function.span = span(property.span);
            }
            // `{ handle: () => {} }` names the function `handle`, for the same reason a
            // variable initialiser does. A prototype setter is the exception: it never reaches
            // the specification's named evaluation, so `{ __proto__: function () {} }` leaves
            // the function anonymous.
            if let kiln_hir::ObjectKey::Static(name) = &key
                && !(name == "__proto__" && form == kiln_hir::PropertyForm::KeyValue)
            {
                name_anonymous_function(&mut value, name);
            }
            members.push(kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                key,
                value,
                form,
                span: span(property.span),
            }));
        }
        HirExpression {
            kind: HirExpressionKind::Object { members },
            span: expression_span,
        }
    }

    fn unsupported_statement(&mut self, feature: &str, feature_span: HirSpan) -> HirStatementKind {
        self.unsupported(feature, feature_span);
        HirStatementKind::Unsupported {
            feature: feature.to_string(),
        }
    }

    fn unsupported_expression(&mut self, feature: &str, feature_span: HirSpan) -> HirExpression {
        self.unsupported(feature, feature_span);
        HirExpression {
            kind: HirExpressionKind::Unsupported {
                feature: feature.to_string(),
            },
            span: feature_span,
        }
    }

    fn unsupported(&mut self, feature: &str, feature_span: HirSpan) {
        // Unsupported-but-valid syntax is represented in executable HIR and diagnosed by the
        // IR compiler. Keeping it out of parser diagnostics lets `kiln check` remain a syntax
        // check even while the execution surface grows.
        let _ = (feature, feature_span);
    }
}

fn lower_import_name(name: &ImportImportName<'_>) -> ImportName {
    match name {
        ImportImportName::Name(name) => ImportName::Name(name.name.to_string()),
        ImportImportName::NamespaceObject => ImportName::Namespace,
        ImportImportName::Default(_) => ImportName::Default,
    }
}

fn lower_export(entry: &OxcExportEntry<'_>) -> Export {
    let exported = match &entry.export_name {
        ExportExportName::Name(name) => Some(ExportName::Name(name.name.to_string())),
        ExportExportName::Default(_) => Some(ExportName::Default),
        ExportExportName::Null => None,
    };
    let source = entry
        .module_request
        .as_ref()
        .map(|request| request.name.to_string());
    let origin = match (source, &entry.import_name, &entry.local_name) {
        (None, ExportImportName::Null, ExportLocalName::Name(local))
        | (None, ExportImportName::Null, ExportLocalName::Default(local)) => ExportOrigin::Local {
            local: local.name.to_string(),
        },
        (None, ExportImportName::Null, ExportLocalName::Null) => ExportOrigin::DefaultExpression,
        (Some(source), ExportImportName::Name(imported), _) => ExportOrigin::ReExport {
            source,
            imported: ImportName::Name(imported.name.to_string()),
        },
        (Some(source), ExportImportName::All, _) => ExportOrigin::NamespaceReExport { source },
        (Some(source), ExportImportName::AllButDefault, _) => ExportOrigin::Star { source },
        (source, import_name, _) => ExportOrigin::Unsupported {
            source,
            description: format!("unexpected export import name {import_name:?}"),
        },
    };
    Export {
        exported,
        origin,
        type_only: entry.is_type,
        span: span(entry.span),
        statement_span: span(entry.statement_span),
    }
}

fn lower_unary_operator(operator: UnaryOperator) -> Option<HirUnaryOperator> {
    match operator {
        UnaryOperator::UnaryPlus => Some(HirUnaryOperator::Plus),
        UnaryOperator::UnaryNegation => Some(HirUnaryOperator::Negate),
        UnaryOperator::LogicalNot => Some(HirUnaryOperator::Not),
        UnaryOperator::Typeof => Some(HirUnaryOperator::TypeOf),
        UnaryOperator::BitwiseNot => Some(HirUnaryOperator::BitwiseNot),
        UnaryOperator::Void => Some(HirUnaryOperator::Void),
        _ => None,
    }
}

/// Every binary operator oxc can produce now has an HIR counterpart, so there is no catch-all
/// here. It keeps returning `Option` to match its unary sibling -- and losing the catch-all means
/// the compiler, rather than a user, reports the day oxc grows another.
fn lower_binary_operator(operator: BinaryOperator) -> Option<HirBinaryOperator> {
    match operator {
        BinaryOperator::Addition => Some(HirBinaryOperator::Add),
        BinaryOperator::Subtraction => Some(HirBinaryOperator::Subtract),
        BinaryOperator::Multiplication => Some(HirBinaryOperator::Multiply),
        BinaryOperator::Division => Some(HirBinaryOperator::Divide),
        BinaryOperator::Remainder => Some(HirBinaryOperator::Remainder),
        BinaryOperator::Exponential => Some(HirBinaryOperator::Exponentiate),
        BinaryOperator::StrictEquality => Some(HirBinaryOperator::StrictEqual),
        BinaryOperator::In => Some(HirBinaryOperator::In),
        BinaryOperator::Instanceof => Some(HirBinaryOperator::InstanceOf),
        BinaryOperator::StrictInequality => Some(HirBinaryOperator::StrictNotEqual),
        BinaryOperator::Equality => Some(HirBinaryOperator::LooseEqual),
        BinaryOperator::Inequality => Some(HirBinaryOperator::LooseNotEqual),
        BinaryOperator::BitwiseAnd => Some(HirBinaryOperator::BitwiseAnd),
        BinaryOperator::BitwiseOR => Some(HirBinaryOperator::BitwiseOr),
        BinaryOperator::BitwiseXOR => Some(HirBinaryOperator::BitwiseXor),
        BinaryOperator::ShiftLeft => Some(HirBinaryOperator::ShiftLeft),
        BinaryOperator::ShiftRight => Some(HirBinaryOperator::ShiftRight),
        BinaryOperator::ShiftRightZeroFill => Some(HirBinaryOperator::UnsignedShiftRight),
        BinaryOperator::LessThan => Some(HirBinaryOperator::LessThan),
        BinaryOperator::LessEqualThan => Some(HirBinaryOperator::LessThanOrEqual),
        BinaryOperator::GreaterThan => Some(HirBinaryOperator::GreaterThan),
        BinaryOperator::GreaterEqualThan => Some(HirBinaryOperator::GreaterThanOrEqual),
    }
}

/// Every assignment operator oxc can produce now has an HIR counterpart, so there is no
/// catch-all -- and losing it means the compiler reports the day oxc grows another.
fn lower_assignment_operator(operator: AssignmentOperator) -> Option<HirAssignmentOperator> {
    match operator {
        AssignmentOperator::Assign => Some(HirAssignmentOperator::Assign),
        AssignmentOperator::Addition => Some(HirAssignmentOperator::Add),
        AssignmentOperator::Subtraction => Some(HirAssignmentOperator::Subtract),
        AssignmentOperator::Multiplication => Some(HirAssignmentOperator::Multiply),
        AssignmentOperator::Division => Some(HirAssignmentOperator::Divide),
        AssignmentOperator::Remainder => Some(HirAssignmentOperator::Remainder),
        AssignmentOperator::Exponential => Some(HirAssignmentOperator::Exponentiate),
        AssignmentOperator::BitwiseAnd => Some(HirAssignmentOperator::BitwiseAnd),
        AssignmentOperator::BitwiseOR => Some(HirAssignmentOperator::BitwiseOr),
        AssignmentOperator::BitwiseXOR => Some(HirAssignmentOperator::BitwiseXor),
        AssignmentOperator::ShiftLeft => Some(HirAssignmentOperator::ShiftLeft),
        AssignmentOperator::ShiftRight => Some(HirAssignmentOperator::ShiftRight),
        AssignmentOperator::ShiftRightZeroFill => Some(HirAssignmentOperator::UnsignedShiftRight),
        AssignmentOperator::LogicalAnd => Some(HirAssignmentOperator::LogicalAnd),
        AssignmentOperator::LogicalOr => Some(HirAssignmentOperator::LogicalOr),
        AssignmentOperator::LogicalNullish => Some(HirAssignmentOperator::Nullish),
    }
}

fn source_kind(source_type: SourceType) -> SourceKind {
    match (source_type.is_typescript(), source_type.is_jsx()) {
        (false, false) => SourceKind::JavaScript,
        (false, true) => SourceKind::Jsx,
        (true, false) => SourceKind::TypeScript,
        (true, true) => SourceKind::Tsx,
    }
}

fn item_kind(statement: &Statement<'_>) -> ItemKind {
    match statement {
        Statement::ImportDeclaration(_) | Statement::TSImportEqualsDeclaration(_) => {
            ItemKind::Import
        }
        Statement::ExportAllDeclaration(_)
        | Statement::ExportDefaultDeclaration(_)
        | Statement::ExportDeclaration(_)
        | Statement::ExportFromDeclaration(_)
        | Statement::ExportNamedDeclaration(_)
        | Statement::TSExportAssignment(_)
        | Statement::TSNamespaceExportDeclaration(_) => ItemKind::Export,
        Statement::VariableDeclaration(_) => ItemKind::Variable,
        Statement::FunctionDeclaration(_) => ItemKind::Function,
        Statement::ClassDeclaration(_) => ItemKind::Class,
        Statement::BlockStatement(_) => ItemKind::Block,
        Statement::ExpressionStatement(_) => ItemKind::Expression,
        Statement::EmptyStatement(_) => ItemKind::Empty,
        Statement::TSTypeAliasDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSExternalModuleDeclaration(_)
        | Statement::TSNamespaceDeclaration(_)
        | Statement::TSGlobalDeclaration(_) => ItemKind::TypeScript,
        Statement::BreakStatement(_)
        | Statement::ContinueStatement(_)
        | Statement::DoWhileStatement(_)
        | Statement::ForInStatement(_)
        | Statement::ForOfStatement(_)
        | Statement::ForStatement(_)
        | Statement::IfStatement(_)
        | Statement::ReturnStatement(_)
        | Statement::SwitchStatement(_)
        | Statement::ThrowStatement(_)
        | Statement::TryStatement(_)
        | Statement::WhileStatement(_) => ItemKind::ControlFlow,
        _ => ItemKind::Other,
    }
}

fn span(value: Span) -> HirSpan {
    HirSpan::new(value.start, value.end)
}

/// Give an anonymous function or class expression the name of the binding it is being assigned
/// to.
///
/// A function's written name becomes a binding inside its own body and an inferred one does not,
/// which is why a class carries the two apart in [`kiln_hir::Class::inferred_name`]: writing an
/// inferred name into `name` would make `var C = class { m() { return C } }` resolve `C` from a
/// scope the program never wrote. With the field there, both kinds of anonymous definition can
/// be named -- and `var C = class {}` stops reporting the empty string, which no engine does.
fn name_anonymous_function(value: &mut kiln_hir::Expression, name: &str) {
    match &mut value.kind {
        kiln_hir::ExpressionKind::Function(function) if function.name.is_none() => {
            function.name = Some(name.to_owned());
        }
        kiln_hir::ExpressionKind::Class(class)
            if class.name.is_none() && class.inferred_name.is_none() =>
        {
            class.inferred_name = Some(name.to_owned());
        }
        _ => {}
    }
}

/// Give a pattern's default the name of the binding it is filling in.
///
/// `var [fn = function () {}] = []` names the function `fn`, exactly as `var fn = function () {}`
/// does: when the source supplies nothing, the default *is* the binding's value, and the
/// specification runs the same named evaluation on it. Only an identifier binding names anything
/// -- a nested pattern spreads its default over several bindings, none of which is the name of
/// the whole.
fn name_anonymous_default(inner: &kiln_hir::Pattern, default: &mut kiln_hir::Expression) {
    if let kiln_hir::Pattern::Identifier(name) = inner {
        name_anonymous_function(default, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commonjs_uses_script_grammar_and_preserves_wrapper_references() {
        let module = parse_commonjs(
            "fixture.cjs",
            "module.exports = function () { return require('./dep.cjs') + __filename.length + __dirname.length + exports.x; };",
        );
        assert_eq!(module.source_goal, SourceGoal::Script);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert!(module.runtime_requested_modules().is_empty());
        assert!(matches!(
            module.body[0].kind,
            HirStatementKind::Expression(HirExpression {
                kind: HirExpressionKind::MemberAssignment { .. },
                ..
            })
        ));
        let (_, requires) = parse_commonjs_with_requires(
            "fixture.cjs",
            "module.exports = function () { return require('./dep.cjs'); };",
        );
        assert_eq!(requires, ["./dep.cjs"]);
    }

    #[test]
    fn js_and_mjs_parse_with_module_goal() {
        let module_js = parse("foo.js", "export const x = 1;");
        assert_eq!(module_js.source_goal, SourceGoal::Module);
        assert!(
            module_js.diagnostics.is_empty(),
            "{:?}",
            module_js.diagnostics
        );

        let with_js = parse("foo.js", "with ({}) {}");
        assert!(
            !with_js.diagnostics.is_empty(),
            "Module goal must reject 'with' statements"
        );

        let module_ts = parse("foo.ts", "export const x: number = 1;");
        assert_eq!(module_ts.source_goal, SourceGoal::Module);

        let cjs = parse("foo.cjs", "var x = 1;");
        assert_eq!(cjs.source_goal, SourceGoal::Script);
    }

    #[test]
    fn commonjs_goal_uses_oxcs_wrapper_grammar() {
        let returned = parse_commonjs("fixture.js", "return module.exports;");
        assert!(
            returned.diagnostics.is_empty(),
            "{:?}",
            returned.diagnostics
        );

        let awaited = parse_commonjs("fixture.js", "await Promise.resolve();");
        assert!(
            !awaited.diagnostics.is_empty(),
            "CommonJS must reject top-level await"
        );
    }

    #[test]
    fn commonjs_requires_use_oxc_scope_semantics() {
        let source = r#"
            require('./first');
            if (ready) require('./nested');
            require('./first');
            require(dynamic);
            function parameter(require) { require('./parameter'); }
            function hoisted() {
                require('./var-before');
                if (ready) var require;
                require('./var-after');
            }
            {
                require('./block-before');
                let require;
                require('./block-after');
            }
            try {} catch (require) { require('./catch'); }
            require('./last');
        "#;
        let (_, requires) = parse_commonjs_with_requires("fixture.cjs", source);
        assert_eq!(requires, ["./first", "./nested", "./last"]);
    }

    #[test]
    fn commonjs_require_discovery_walks_all_oxc_ast_nodes_in_source_order() {
        let source = r#"
            const value = {
                [require('./key')]: require('./value'),
                method() { return require('./method'); }
            };
            class Example extends require('./base') {
                field = require('./field');
            }
            [require('./array'), ...require('./spread')];
        "#;
        let (_, requires) = parse_commonjs_with_requires("fixture.cjs", source);
        assert_eq!(
            requires,
            [
                "./key", "./value", "./method", "./base", "./field", "./array", "./spread"
            ]
        );
    }

    #[test]
    fn parses_real_javascript_export_forms_without_scanning_comments_or_strings() {
        let module = parse(
            "module.js",
            r#"
                const local = 1;
                const text = "export const fake = 1";
                // export const commented = 2;
                export { local as answer };
                export default function () {}
            "#,
        );

        assert!(!module.panicked);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["answer", "default"]);
    }

    #[test]
    fn parses_typescript_and_excludes_type_only_exports_from_runtime_interface() {
        let module = parse(
            "module.ts",
            r#"
                export interface Shape { width: number }
                export type Id = string;
                export const value: Id = "ok";
            "#,
        );

        assert_eq!(module.source_kind, SourceKind::TypeScript);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["value"]);
    }

    #[test]
    fn records_dependencies_and_star_reexports() {
        let module = parse(
            "module.mjs",
            "import { a } from './a.js';\nexport * from './b.js';\n",
        );

        assert_eq!(
            module
                .requested_modules
                .iter()
                .map(|request| request.specifier.as_str())
                .collect::<Vec<_>>(),
            ["./a.js", "./b.js"]
        );
        assert!(module.exports.iter().any(|export| {
            export.exported.is_none()
                && matches!(
                    &export.origin,
                    ExportOrigin::Star { source } if source == "./b.js"
                )
        }));
        assert_eq!(module.imports.len(), 1);
        assert_eq!(module.imports[0].source, "./a.js");
        assert_eq!(module.imports[0].local, "a");
        assert_eq!(module.imports[0].imported, ImportName::Name("a".into()));
        assert_eq!(
            module.requested_modules[0].occurrences[0].kind,
            ModuleRequestKind::Import
        );
        assert_eq!(
            module.requested_modules[1].occurrences[0].kind,
            ModuleRequestKind::Export
        );
    }

    #[test]
    fn preserves_aliases_defaults_namespaces_and_type_only_bindings() {
        let module = parse(
            "module.ts",
            r#"
                import primary, { value as local, type Shape } from "./dep.ts";
                import * as namespace from "./other.ts";
                const result = local;
                export { result as answer };
                export { value as forwarded } from "./dep.ts";
            "#,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.imports.len(), 4);
        assert!(module.imports.iter().any(|entry| {
            entry.local == "primary" && entry.imported == ImportName::Default && !entry.type_only
        }));
        assert!(module.imports.iter().any(|entry| {
            entry.local == "local"
                && entry.imported == ImportName::Name("value".into())
                && !entry.type_only
        }));
        assert!(module.imports.iter().any(|entry| {
            entry.local == "Shape"
                && entry.imported == ImportName::Name("Shape".into())
                && entry.type_only
        }));
        assert!(module.imports.iter().any(|entry| {
            entry.local == "namespace" && entry.imported == ImportName::Namespace
        }));
        assert!(module.exports.iter().any(|entry| {
            entry.exported == Some(ExportName::Name("answer".into()))
                && entry.origin
                    == ExportOrigin::Local {
                        local: "result".into(),
                    }
        }));
        assert!(module.exports.iter().any(|entry| {
            entry.exported == Some(ExportName::Name("forwarded".into()))
                && entry.origin
                    == ExportOrigin::ReExport {
                        source: "./dep.ts".into(),
                        imported: ImportName::Name("value".into()),
                    }
        }));
    }

    #[test]
    fn default_expression_is_not_lowered_as_a_discarded_expression() {
        let module = parse("module.js", "export default 40 + 2;");

        assert!(matches!(
            module.body.as_slice(),
            [HirStatement {
                kind: HirStatementKind::ExportDefault(_),
                ..
            }]
        ));
        assert!(matches!(
            module.exports[0].origin,
            ExportOrigin::DefaultExpression
        ));
    }

    #[test]
    fn an_assigning_loop_head_binds_a_hidden_name_and_assigns_it_in_the_body() {
        // The shape is the correctness argument: the hidden binding must be `let` so it is fresh
        // each iteration, the assignment must be the body's first statement so the target's
        // reference is taken once and only after the step's value exists, and the source's body
        // must stay a statement of its own so a `let` in it cannot shadow the target out from
        // under the assignment above.
        for (source, prefix) in [
            ("for (target of values) { body(); }", "%forof"),
            ("for (target in values) { body(); }", "%forin"),
        ] {
            let module = parse("module.js", source);
            assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);

            let (kind, hidden, body) = match &module.body[0].kind {
                HirStatementKind::ForOf {
                    kind,
                    pattern,
                    body,
                    ..
                } => match pattern {
                    kiln_hir::Pattern::Identifier(name) => (kind, name, body),
                    other => panic!("expected an identifier pattern, got {other:?}"),
                },
                HirStatementKind::ForIn {
                    kind, name, body, ..
                } => (kind, name, body),
                other => panic!("expected a loop for {source:?}, got {other:?}"),
            };
            assert_eq!(*kind, HirVariableKind::Let);
            assert!(
                hidden.starts_with(prefix),
                "{hidden} should start with {prefix}"
            );

            let HirStatementKind::Block(statements) = &body.kind else {
                panic!("expected a block body, got {:?}", body.kind);
            };
            assert_eq!(statements.len(), 2, "assignment then the source's own body");
            assert!(matches!(
                &statements[0].kind,
                HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Assignment { target, value, .. },
                    ..
                }) if target == "target"
                    && matches!(&value.kind, HirExpressionKind::Identifier(name) if name == hidden)
            ));
            assert!(matches!(statements[1].kind, HirStatementKind::Block(_)));
        }
    }

    #[test]
    fn unsupported_expression_diagnostics_name_the_oxc_variant() {
        // This list shrinks as constructs get lowered -- object and `new` expressions were both
        // here and both left. That churn is the point rather than a nuisance: what the test
        // protects is that whatever *remains* unsupported names the construct, instead of
        // falling into a catch-all reporting the bare word "expression". A third of the guard
        // corpus was once hidden behind exactly that, which is why it is worth a test at all.
        // If a case here starts failing, the construct was implemented: delete the line.
        let cases = [(
            "const value = import.defer(\"./x.js\");",
            "phased dynamic import expression",
        )];
        for (source, expected) in cases {
            let module = parse("module.js", source);
            let HirStatementKind::Variable { declarations, .. } = &module.body[0].kind else {
                panic!("expected variable declaration for {source:?}");
            };
            let initializer = declarations[0]
                .initializer
                .as_ref()
                .expect("fixture has an initializer");
            assert!(matches!(
                &initializer.kind,
                HirExpressionKind::Unsupported { feature } if feature == expected
            ));
        }
    }

    #[test]
    fn a_refused_class_names_the_member_that_refused_it() {
        // A class is refused whole, so every cause used to arrive as the wrapper "class
        // expression" -- one bucket covering an accessor property, an async method and a
        // computed key alike, and 2,294 guard-corpus cases behind it with nothing to rank them
        // by. (The static block that used to stand here lowers now.) What is
        // protected here is that the reported name describes the *member*, not the syntactic
        // position the class sat in. If a case here starts failing, the member was lowered:
        // delete the line.
        let module = parse("module.js", "const value = class { accessor #x = 1; };");
        let HirStatementKind::Variable { declarations, .. } = &module.body[0].kind else {
            panic!("expected a variable declaration");
        };
        let initializer = declarations[0]
            .initializer
            .as_ref()
            .expect("fixture has an initializer");
        assert!(matches!(
            &initializer.kind,
            HirExpressionKind::Unsupported { feature }
                if feature == "private class accessor property"
        ));

        // The declaration form reports the same name: the two callers differ only in what they
        // wrap it in, which is what made them report two different useless ones.
        let module = parse("module.js", "class Holder { accessor #x = 1; }");
        assert!(matches!(
            &module.body[0].kind,
            HirStatementKind::Unsupported { feature }
                if feature == "private class accessor property"
        ));
    }

    #[test]
    fn runtime_requests_exclude_inline_type_only_bindings_but_keep_side_effects() {
        let module = parse(
            "module.ts",
            r#"
                import { type Shape } from "./shape.ts";
                export { type Id } from "./id.ts";
                import "./side-effect.ts";
            "#,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(
            module
                .runtime_requested_modules()
                .into_iter()
                .map(|request| request.specifier.as_str())
                .collect::<Vec<_>>(),
            ["./side-effect.ts"]
        );
    }

    #[test]
    fn syntax_errors_are_owned_diagnostics_instead_of_panics() {
        let module = parse("broken.ts", "export const value: = ;");

        assert!(module.panicked || !module.diagnostics.is_empty());
        assert!(
            module.panicked
                || module
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.span.is_some()),
            "recoverable diagnostics should retain a useful source range: {:?}",
            module.diagnostics
        );
    }

    #[test]
    fn recognises_tsx_from_the_path() {
        let module = parse("component.tsx", "export const view = <div />;");
        assert_eq!(module.source_kind, SourceKind::Tsx);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
    }

    #[test]
    fn explicit_goal_overrides_the_javascript_extension() {
        let script = parse_with_goal("case.js", "var await = 1;", SourceGoal::Script);
        let module = parse_with_goal("case.js", "var await = 1;", SourceGoal::Module);

        assert_eq!(script.source_goal, SourceGoal::Script);
        assert!(script.diagnostics.is_empty(), "{:?}", script.diagnostics);
        assert_eq!(module.source_goal, SourceGoal::Module);
        assert!(
            !module.diagnostics.is_empty(),
            "module goal must reject an await binding"
        );
    }

    #[test]
    fn early_errors_are_reported_even_though_the_grammar_accepts_them() {
        // Each of these parses as a well-formed production and is still a `SyntaxError` before
        // evaluation begins, so the parser alone cannot decide them.
        for source in [
            "\"use strict\"; (eval) = 20;",
            "class C { x = () => arguments; }",
            "let x; const x = 0;",
            "function f() { \"use strict\"; delete x; }",
        ] {
            let module = parse_with_goal("case.js", source, SourceGoal::Script);
            assert!(
                !module.diagnostics.is_empty(),
                "early error accepted: {source}"
            );
            assert!(
                module
                    .diagnostics
                    .iter()
                    .all(|diagnostic| diagnostic.span.is_some()),
                "an early error must point at source: {:?}",
                module.diagnostics
            );
        }
    }

    #[test]
    fn invalid_regular_expression_patterns_are_syntax_errors() {
        // The flags are checked by the tokenizer either way; these fail only because the pattern
        // itself is parsed.  `\\p{ASCII=F}` is well-formed as a *flagless* pattern and invalid
        // under `u`, so the flags have to reach the pattern parser too.
        for source in [
            "/(/;",
            "/(?<a>x)(?<a>y)/;",
            "/\\p{ASCII=F}/u;",
            "/[b-a]/;",
            "/x{2,1}/;",
            "/\\2/u;",
        ] {
            let module = parse_with_goal("case.js", source, SourceGoal::Script);
            assert!(
                !module.diagnostics.is_empty(),
                "invalid pattern accepted: {source}"
            );
        }
    }

    #[test]
    fn permissive_regular_expression_patterns_still_parse() {
        // Over-rejection is the whole risk of parsing patterns, and Annex B is where it hides:
        // a bare `{`, an unmatched `]`, an empty class, and an octal escape are all legal in a
        // pattern without `u`/`v` and illegal with it.  The named-group forms guard the other
        // direction, where a valid construct looks malformed.
        for source in [
            "/{/;",
            "/]/;",
            "/[]/;",
            "/[^]/;",
            "/\\101/;",
            "/(?<a>x)\\k<a>/;",
            "/a{,2}/;",
            "/[\\w-a]/;",
            "/\\p{ASCII}/u;",
            "/[\\p{ASCII}--[a]]/v;",
            "/(?<\\u0061>x)/;",
        ] {
            let script = parse_with_goal("case.js", source, SourceGoal::Script);
            assert!(
                script.diagnostics.is_empty(),
                "valid pattern rejected: {source}: {:?}",
                script.diagnostics
            );
        }
    }

    #[test]
    fn an_early_error_follows_the_goal_it_was_parsed_under() {
        // `delete` of a bare reference is legal sloppy script and a `SyntaxError` in a module,
        // because a module is strict.  Test262 runs the same file both ways, so a check that
        // ignored the goal would answer one of the two variants wrongly.
        let script = parse_with_goal("case.js", "var x; delete x;", SourceGoal::Script);
        let module = parse_with_goal("case.js", "var x; delete x;", SourceGoal::Module);

        assert!(script.diagnostics.is_empty(), "{:?}", script.diagnostics);
        assert!(
            !module.diagnostics.is_empty(),
            "a module is strict, so deleting a binding is an early error"
        );
    }

    #[test]
    fn ordinary_programs_keep_parsing_clean() {
        // The whole risk of a semantic pass is over-rejection, so pin the forms that look like
        // redeclarations and are not: repeated `var`, a function shadowing its own name, and a
        // block-scoped binding that merely shadows an outer one.
        for source in [
            "var a = 1; var a = 2; function f(a) { return a; }",
            "(function n(n) { return n; });",
            "let a = 1; { let a = 2; } for (let a of []) a;",
            "try { null; } catch (e) { let f = 1; }",
        ] {
            let script = parse_with_goal("case.js", source, SourceGoal::Script);
            assert!(
                script.diagnostics.is_empty(),
                "{source}: {:?}",
                script.diagnostics
            );
        }
    }

    #[test]
    fn lowers_the_interpreter_language_without_retaining_oxc_values() {
        let module = parse(
            "main.ts",
            r#"
                let total: number = 1;
                while (total < 4) total += 1;
                if (total === 4) console.log("total", total);
            "#,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.body.len(), 3);
        assert!(matches!(
            module.body[0].kind,
            HirStatementKind::Variable { .. }
        ));
        assert!(matches!(
            module.body[1].kind,
            HirStatementKind::While { .. }
        ));
        assert!(matches!(module.body[2].kind, HirStatementKind::If { .. }));
    }

    /// The parameters of the single function a module declares.
    fn parameters_of(source: &str) -> (Vec<kiln_hir::Parameter>, Option<kiln_hir::Pattern>) {
        let module = parse("parameters.ts", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let Some(kiln_hir::StatementKind::Function(function)) =
            module.body.first().map(|statement| &statement.kind)
        else {
            panic!("expected one function declaration");
        };
        (function.parameters.clone(), function.rest.clone())
    }

    /// The name the first default inside a pattern gives its function, if it is one.
    ///
    /// `None` distinguishes "no function there" from `Some(None)`, "a function left anonymous".
    fn defaulted_function_name(pattern: &kiln_hir::Pattern) -> Option<Option<String>> {
        match pattern {
            kiln_hir::Pattern::WithDefault { default, .. } => match &default.kind {
                HirExpressionKind::Function(function) => Some(function.name.clone()),
                _ => None,
            },
            kiln_hir::Pattern::Array { elements, .. } => {
                elements.iter().flatten().find_map(defaulted_function_name)
            }
            kiln_hir::Pattern::Object { properties, .. } => properties
                .iter()
                .find_map(|property| defaulted_function_name(&property.value)),
            _ => None,
        }
    }

    /// The name given to the function in the first default of the first statement's pattern.
    fn defaulted_name(source: &str) -> Option<String> {
        let module = parse_with_goal("default.js", source, SourceGoal::Script);
        assert!(
            module.diagnostics.is_empty(),
            "{source}: {:?}",
            module.diagnostics
        );
        let pattern = match module.body.last().map(|statement| &statement.kind) {
            Some(HirStatementKind::Variable { declarations, .. }) => {
                declarations[0].pattern.clone()
            }
            Some(HirStatementKind::Function(function)) => function.parameters[0].pattern.clone(),
            Some(HirStatementKind::Expression(expression)) => match &expression.kind {
                HirExpressionKind::PatternAssignment { pattern, .. } => pattern.clone(),
                other => panic!("{source}: unexpected expression {other:?}"),
            },
            other => panic!("{source}: unexpected statement {other:?}"),
        };
        defaulted_function_name(&pattern).expect("a function in a default")
    }

    #[test]
    fn a_default_names_the_anonymous_function_it_stands_in_for() {
        // Named evaluation reaches a default, not only an initialiser: when the source supplies
        // nothing, the default *is* the binding's value, so `var [fn = function () {}] = []`
        // has to leave `fn.name` as `"fn"`. Every shape below took its own construction site in
        // this frontend, and each one that was missed read back an empty name -- a value no
        // engine produces.
        for (source, expected) in [
            ("var [fn = function () {}] = [];", Some("fn")),
            ("var { p: fn = () => {} } = {};", Some("fn")),
            ("var { fn = function* () {} } = {};", Some("fn")),
            ("let [fn = async function () {}] = [];", Some("fn")),
            ("function outer(fn = function () {}) {}", Some("fn")),
            ("var fn; [fn = function () {}] = [];", Some("fn")),
            // A parenthesized target is a cover grammar, not the identifier reference the
            // specification names through.
            ("var fn; [(fn) = function () {}] = [];", None),
            ("var fn; ({ p: (fn) = function () {} } = {});", None),
            ("var fn; ({ fn = function () {} } = {});", Some("fn")),
            ("var fn; ({ p: fn = function () {} } = {});", Some("fn")),
            // A written name is the function's own and is visible inside its body, so inferring
            // over it would rename a function that can call itself.
            ("var [fn = function named() {}] = [];", Some("named")),
            // A nested pattern spreads the default across several bindings, and none of them is
            // the name of the whole.
            ("var [[a, b] = function () {}] = [];", None),
        ] {
            assert_eq!(defaulted_name(source).as_deref(), expected, "{source}");
        }
    }

    #[test]
    fn assigning_to_an_identifier_names_the_anonymous_function() {
        // `fn = function () {}` is a named evaluation just as `var fn = function () {}` is. The
        // logical forms qualify because they supply the binding's whole value; `+=` does not,
        // because it combines the function with what the binding already held.
        for (source, expected) in [
            ("fn = function () {};", Some("fn")),
            ("fn = () => {};", Some("fn")),
            ("fn ||= function () {};", Some("fn")),
            ("fn &&= function () {};", Some("fn")),
            ("fn ??= function () {};", Some("fn")),
            ("fn = function named() {};", Some("named")),
            // A parenthesized target is a cover grammar rather than an identifier reference,
            // and the specification names nothing through one.
            ("(fn) = function () {};", None),
            ("((fn)) = function () {};", None),
            ("fn += function () {};", None),
            // Only an identifier target names anything: a property already has a name of its
            // own, and the specification does not rename what is stored into it.
            ("o.fn = function () {};", None),
        ] {
            let module = parse_with_goal("assign.js", source, SourceGoal::Script);
            assert!(
                module.diagnostics.is_empty(),
                "{source}: {:?}",
                module.diagnostics
            );
            let Some(HirStatementKind::Expression(expression)) =
                module.body.first().map(|statement| &statement.kind)
            else {
                panic!("{source}: expected one expression statement");
            };
            let value = match &expression.kind {
                HirExpressionKind::Assignment { value, .. } => value,
                HirExpressionKind::MemberAssignment { value, .. } => value,
                other => panic!("{source}: unexpected expression {other:?}"),
            };
            let HirExpressionKind::Function(function) = &value.kind else {
                panic!("{source}: expected a function on the right");
            };
            assert_eq!(function.name.as_deref(), expected, "{source}");
        }
    }

    #[test]
    fn named_evaluation_infers_an_anonymous_class_name_without_creating_a_binding() {
        let module = parse_with_goal("class-name.js", "let C = class {};", SourceGoal::Script);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let HirStatementKind::Variable { declarations, .. } = &module.body[0].kind else {
            panic!("expected a variable declaration");
        };
        let Some(initializer) = &declarations[0].initializer else {
            panic!("expected a class initializer");
        };
        let HirExpressionKind::Class(class) = &initializer.kind else {
            panic!("expected a class initializer, got {:?}", initializer.kind);
        };
        assert_eq!(class.name, None);
        assert_eq!(class.inferred_name.as_deref(), Some("C"));
    }

    #[test]
    fn optional_private_member_lowering_preserves_the_short_circuit_marker() {
        let module = parse_with_goal(
            "optional-private.js",
            "class C { #x; read(value) { return value?.#x; } }",
            SourceGoal::Script,
        );
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let HirStatementKind::Class(class) = &module.body[0].kind else {
            panic!("expected a class declaration");
        };
        let method = class
            .methods
            .iter()
            .find(|method| matches!(method.key, kiln_hir::ObjectKey::Static(ref name) if name == "read"))
            .expect("read method");
        let Some(statement) = method.function.body.first() else {
            panic!("expected a return statement");
        };
        let HirStatementKind::Return(Some(value)) = &statement.kind else {
            panic!("expected a return expression");
        };
        let HirExpressionKind::Chain { expression } = &value.kind else {
            panic!("expected an optional chain, got {:?}", value.kind);
        };
        let HirExpressionKind::PrivateMember { optional, .. } = &expression.kind else {
            panic!(
                "expected an optional private member, got {:?}",
                expression.kind
            );
        };
        assert!(*optional);
    }

    #[test]
    fn a_parameter_default_survives_lowering() {
        // oxc keeps the default on the `FormalParameter`, not inside the binding pattern, so a
        // lowering that reads only the pattern sees a bare identifier and drops the default in
        // silence -- which is how `function a(x = 5) { return x }` came to return undefined.
        let (parameters, _) = parameters_of("function a(x = 5) { return x; }");
        let kiln_hir::Pattern::WithDefault { inner, default } = &parameters[0].pattern else {
            panic!("expected a default, got {:?}", parameters[0].pattern);
        };
        assert_eq!(**inner, kiln_hir::Pattern::Identifier("x".to_owned()));
        assert!(matches!(
            default.kind,
            kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::Number(_))
        ));
    }

    #[test]
    fn a_parameter_binds_whatever_pattern_was_written() {
        let (parameters, _) = parameters_of("function a(plain, { key }, [item]) {}");
        assert!(matches!(
            parameters[0].pattern,
            kiln_hir::Pattern::Identifier(_)
        ));
        assert!(matches!(
            parameters[1].pattern,
            kiln_hir::Pattern::Object { .. }
        ));
        assert!(matches!(
            parameters[2].pattern,
            kiln_hir::Pattern::Array { .. }
        ));
    }

    #[test]
    fn a_rest_parameter_is_separate_from_the_positional_ones() {
        // It takes no positional slot and its arity is the caller's, so it cannot be one of them.
        let (parameters, rest) = parameters_of("function a(first, ...others) {}");
        assert_eq!(parameters.len(), 1);
        assert_eq!(
            rest,
            Some(kiln_hir::Pattern::Identifier("others".to_owned()))
        );
        let (_, none) = parameters_of("function a(first) {}");
        assert_eq!(none, None);
    }

    /// A direct `eval` is parsed in the syntactic context of its call site.
    ///
    /// Each of these three is refused outright in a Script and is legal in eval code written
    /// inside the right construct.  The pairs matter as much as the permissions: the refusal has
    /// to survive where the language keeps it, or the suppression has stopped being a context
    /// and become a blanket exemption.
    #[test]
    fn eval_syntax_permits_only_what_the_call_site_allows() {
        let none = EvalSyntax::default();
        for source in ["new.target;", "super.x;", "this.#m;"] {
            assert!(
                !parse_eval("<eval>", source, &none).diagnostics.is_empty(),
                "`{source}` is a SyntaxError in eval code written at the top level of a script"
            );
        }

        let in_function = EvalSyntax {
            new_target: true,
            ..EvalSyntax::default()
        };
        assert_eq!(
            parse_eval("<eval>", "new.target;", &in_function).diagnostics,
            Vec::new(),
            "`new.target` is legal in eval code written inside a function"
        );
        // Still a Script, and everything else it forbids it still forbids: a permission that
        // widened past its own construct would be indistinguishable from not parsing at all.
        assert!(
            !parse_eval("<eval>", "new.target; super.x;", &in_function)
                .diagnostics
                .is_empty()
        );

        let in_method = EvalSyntax {
            super_property: true,
            ..EvalSyntax::default()
        };
        assert_eq!(
            parse_eval("<eval>", "super.x;", &in_method).diagnostics,
            Vec::new(),
            "`super.x` is legal in eval code written inside a method"
        );
        // `super()` is a different production with a different rule, and being inside a method
        // does not make it legal -- Test262 asks for exactly this.
        assert!(
            !parse_eval("<eval>", "super();", &in_method)
                .diagnostics
                .is_empty()
        );

        let in_class = EvalSyntax {
            private_names: vec!["m".to_owned()],
            ..EvalSyntax::default()
        };
        assert_eq!(
            parse_eval("<eval>", "this.#m;", &in_class).diagnostics,
            Vec::new(),
            "a private name the enclosing class declared is visible to a direct eval"
        );
        // Per name, not per source: a name no enclosing class declared is still a SyntaxError.
        assert!(
            !parse_eval("<eval>", "this.#absent;", &in_class)
                .diagnostics
                .is_empty()
        );
    }

    /// A permitted parser diagnostic must not swallow the early errors that come after it.
    ///
    /// Oxc raises `new.target` from the parser and runs its early-error pass only over a source
    /// the parser accepted.  Dropping the diagnostic after that pass had already been skipped
    /// would make `eval` inside a function accept `let x; const x = 0;`.
    #[test]
    fn a_permitted_parser_diagnostic_still_leaves_the_early_errors_in_place() {
        let in_function = EvalSyntax {
            new_target: true,
            ..EvalSyntax::default()
        };
        assert!(
            !parse_eval("<eval>", "new.target; let x; const x = 0;", &in_function)
                .diagnostics
                .is_empty()
        );
    }

    #[test]
    fn typescript_parameter_and_return_annotations_are_extracted() {
        use kiln_hir::TypeAnnotation;

        let module = parse(
            "dist.ts",
            "function dist(x: number, y: number): number { return x + y; }",
        );
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let Some(kiln_hir::StatementKind::Function(dist_fn)) = module.body.first().map(|s| &s.kind)
        else {
            panic!("expected function statement");
        };
        assert_eq!(dist_fn.parameters.len(), 2);
        assert_eq!(
            dist_fn.parameters[0].type_annotation,
            Some(TypeAnnotation::Number)
        );
        assert_eq!(
            dist_fn.parameters[1].type_annotation,
            Some(TypeAnnotation::Number)
        );
        assert_eq!(dist_fn.return_type, Some(TypeAnnotation::Number));

        let module2 = parse(
            "arrow.ts",
            "const f = (a: boolean, b: string, c: int): void => {};",
        );
        assert!(module2.diagnostics.is_empty(), "{:?}", module2.diagnostics);
        let Some(kiln_hir::StatementKind::Variable { declarations, .. }) =
            module2.body.first().map(|s| &s.kind)
        else {
            panic!("expected variable statement");
        };
        let Some(kiln_hir::ExpressionKind::Function(arrow_fn)) =
            declarations[0].initializer.as_ref().map(|i| &i.kind)
        else {
            panic!("expected arrow function");
        };
        assert!(arrow_fn.arrow);
        assert_eq!(arrow_fn.parameters.len(), 3);
        assert_eq!(
            arrow_fn.parameters[0].type_annotation,
            Some(TypeAnnotation::Boolean)
        );
        assert_eq!(
            arrow_fn.parameters[1].type_annotation,
            Some(TypeAnnotation::String)
        );
        assert_eq!(
            arrow_fn.parameters[2].type_annotation,
            Some(TypeAnnotation::Int)
        );
        assert_eq!(arrow_fn.return_type, Some(TypeAnnotation::Void));

        let module3 = parse("untyped.ts", "function untyped(a, b) {}");
        assert!(module3.diagnostics.is_empty(), "{:?}", module3.diagnostics);
        let Some(kiln_hir::StatementKind::Function(untyped_fn)) =
            module3.body.first().map(|s| &s.kind)
        else {
            panic!("expected function statement");
        };
        assert_eq!(untyped_fn.parameters[0].type_annotation, None);
        assert_eq!(untyped_fn.parameters[1].type_annotation, None);
        assert_eq!(untyped_fn.return_type, None);

        let module4 = parse(
            "custom.ts",
            "function custom(x: MyType, y: any): CustomReturn {}",
        );
        assert!(module4.diagnostics.is_empty(), "{:?}", module4.diagnostics);
        let Some(kiln_hir::StatementKind::Function(custom_fn)) =
            module4.body.first().map(|s| &s.kind)
        else {
            panic!("expected function statement");
        };
        assert_eq!(
            custom_fn.parameters[0].type_annotation,
            Some(TypeAnnotation::Other)
        );
        assert_eq!(
            custom_fn.parameters[1].type_annotation,
            Some(TypeAnnotation::Any)
        );
        assert_eq!(custom_fn.return_type, Some(TypeAnnotation::Other));
    }

    #[test]
    fn test_parse_jsx_module() {
        let source = "const x = <div id=\"test\"><span>hello</span><>{123}</></div>;\nexport function App() { return <h1>Title</h1>; }";
        let module = parse("test.tsx", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert!(
            module
                .requested_modules
                .iter()
                .any(|r| r.specifier == "react/jsx-runtime")
        );
        assert!(
            module
                .imports
                .iter()
                .any(|i| i.source == "react/jsx-runtime" && i.local == "_jsx")
        );
        assert!(
            module
                .imports
                .iter()
                .any(|i| i.source == "react/jsx-runtime" && i.local == "_Fragment")
        );
        assert_eq!(module.source_kind, SourceKind::Tsx);
        assert!(!module.body.is_empty());
    }

    #[test]
    fn test_parse_jsx_commonjs() {
        let source = "const x = <div id=\"test\">hello</div>;\n";
        let (module, requires) = parse_commonjs_with_requires("test.tsx", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert!(
            requires.contains(&"react/jsx-runtime".to_string()),
            "expected react/jsx-runtime in requires: {:?}",
            requires
        );
    }

    #[test]
    fn test_tsconfig_jsonc_cleaning() {
        let tsconfig = r#"{
            // Single line comment
            "compilerOptions": {
                /* Multi-line
                   comment */
                "jsx": "react-jsx",
                "jsxImportSource": "@opentui/react",
            },
        }"#;
        let options = parse_tsconfig_jsx(tsconfig).expect("valid tsconfig");
        assert_eq!(options.runtime, JsxRuntime::Automatic);
        assert_eq!(options.import_source.as_deref(), Some("@opentui/react"));
    }

    #[test]
    fn test_parse_import_meta() {
        let source = "const url = import.meta.url;\nconst dir = import.meta.dirname;\n";
        let module = parse("/path/to/module.js", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.body.len(), 2);
    }
}

//! Oxc adapter for Kiln's owned HIR.
//!
//! This is the only crate allowed to expose Oxc to the compiler build graph. Its public API
//! accepts ordinary strings and returns [`kiln_hir`] values, so an Oxc upgrade cannot leak AST
//! types or arena lifetimes into the query database, interpreter, or backends.

mod dce_guard;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use kiln_hir::{
    AssignmentOperator as HirAssignmentOperator, BinaryOperator as HirBinaryOperator, Diagnostic,
    Export, ExportName, ExportOrigin, Expression as HirExpression,
    ExpressionKind as HirExpressionKind, Function as HirFunction, Import, ImportName, Item,
    ItemKind, ListElement, Literal as HirLiteral, LogicalOperator as HirLogicalOperator, Module,
    ModuleRequestKind, ModuleRequestOccurrence, NodeRuntimeRequest, Parameter as HirParameter,
    RequestedModule, SourceGoal, SourceKind, Span as HirSpan, Statement as HirStatement,
    StatementKind as HirStatementKind, StringUnits, TypeAnnotation as HirTypeAnnotation,
    TypeAnnotationSite as HirTypeAnnotationSite, TypeDeclaration as HirTypeDeclaration,
    TypeDeclarationKind as HirTypeDeclarationKind, TypeExpr as HirTypeExpr,
    TypeExprKind as HirTypeExprKind, TypeSite as HirTypeSite, TypeSyntax as HirTypeSyntax,
    UnaryOperator as HirUnaryOperator, VariableDeclarator as HirVariableDeclarator,
    VariableKind as HirVariableKind,
};
use oxc_allocator::{Allocator, TakeIn};
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
use oxc_ast_visit::{Visit, walk};
use oxc_diagnostics::OxcDiagnostic;
use oxc_ecmascript::BoundNames;
use oxc_minifier::{CompressOptions, Compressor};
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SymbolId;
use oxc_semantic::{IsGlobalReference, SemanticBuilder};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::module_record::{
    ExportEntry as OxcExportEntry, ExportExportName, ExportImportName, ExportLocalName,
    ImportImportName,
};
use oxc_syntax::operator::UpdateOperator;
use oxc_syntax::operator::{AssignmentOperator, BinaryOperator, LogicalOperator, UnaryOperator};
use oxc_transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer};

/// Convert Oxc's lossless sentinel representation into ECMAScript UTF-16 units.
///
/// Oxc stores a lone surrogate in its otherwise-UTF-8 `Str` as U+FFFD followed by four
/// hexadecimal digits, and uses U+FFFDfffd for a real replacement character when the
/// `lone_surrogates` bit is set. This is the representation documented by Oxc's AST serializer;
/// it is not JavaScript source decoding and remains confined to this adapter.
fn oxc_string_units(value: &str, lone_surrogates: bool) -> StringUnits {
    if !lone_surrogates {
        return StringUnits::from(value);
    }
    let chars: Vec<char> = value.chars().collect();
    let mut units = Vec::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\u{fffd}' && index + 4 < chars.len() {
            let digits: String = chars[index + 1..index + 5].iter().collect();
            if let Ok(unit) = u16::from_str_radix(&digits, 16) {
                units.push(unit);
                index += 5;
                continue;
            }
        }
        let mut encoded = [0; 2];
        units.extend(chars[index].encode_utf16(&mut encoded).iter().copied());
        index += 1;
    }
    StringUnits::from_units(&units)
}

/// Lossless equivalent of Oxc's `PropertyKey::static_name()`. The latter intentionally exposes
/// only `&str`, so it cannot carry the AST node's `lone_surrogates` bit for quoted or template
/// property keys.
fn oxc_static_key_units(key: &oxc_ast::ast::PropertyKey<'_>) -> Option<StringUnits> {
    if let Some(expression) = key.as_expression() {
        match expression {
            Expression::StringLiteral(literal) => {
                return Some(oxc_string_units(
                    literal.value.as_str(),
                    literal.lone_surrogates,
                ));
            }
            Expression::TemplateLiteral(literal) if literal.expressions.is_empty() => {
                let quasi = literal.quasis.first()?;
                return Some(oxc_string_units(
                    quasi.value.cooked.as_ref()?.as_str(),
                    quasi.lone_surrogates,
                ));
            }
            // A numeric literal key is `ToString` of its value, not its source
            // spelling: `0.0000001` and `1e-7` name the same property.
            Expression::NumericLiteral(literal) => {
                return Some(StringUnits::from(
                    kiln_host_abi::number_to_js_string(literal.value).as_str(),
                ));
            }
            _ => {}
        }
    }
    key.static_name()
        .map(|name| StringUnits::from(name.as_ref()))
}

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

/// Discover package-like source specifiers without constructing the owned executable HIR.
///
/// This is intended for source synchronization and metadata discovery, not diagnostics: callers
/// that need parser errors must use [`parse`]. Oxc's module record supplies static imports and
/// exports; the small visitor adds TypeScript import types and external `import = require(...)`
/// declarations, which are type-resolution edges rather than ECMAScript module requests.
pub fn discover_module_specifiers(path: &str, source: &str) -> Vec<String> {
    let mut source_type = SourceType::from_path(Path::new(path))
        .unwrap_or_else(|_| SourceType::default().with_module(true));
    if !path.ends_with(".cjs") {
        source_type = source_type.with_module(true);
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();

    #[derive(Default)]
    struct TypeRequestCollector {
        specifiers: BTreeSet<String>,
    }
    impl<'a> Visit<'a> for TypeRequestCollector {
        fn visit_ts_import_type(&mut self, node: &oxc_ast::ast::TSImportType<'a>) {
            self.specifiers.insert(node.source.value.to_string());
            walk::walk_ts_import_type(self, node);
        }

        fn visit_ts_import_equals_declaration(
            &mut self,
            node: &oxc_ast::ast::TSImportEqualsDeclaration<'a>,
        ) {
            if let oxc_ast::ast::TSModuleReference::ExternalModuleReference(reference) =
                &node.module_reference
            {
                self.specifiers
                    .insert(reference.expression.value.to_string());
            }
            walk::walk_ts_import_equals_declaration(self, node);
        }
    }

    let mut specifiers = parsed
        .module_record
        .requested_modules
        .keys()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    let mut type_requests = TypeRequestCollector::default();
    type_requests.visit_program(&parsed.program);
    specifiers.extend(type_requests.specifiers);

    // Triple-slash `types` references are package-resolution edges. `path` is a source-relative
    // file reference and `lib` names a built-in library, so neither belongs in this package list.
    for comment in &parsed.program.comments {
        if comment.kind != oxc_ast::ast::CommentKind::Line {
            continue;
        }
        let start = (comment.span.start as usize).min(source.len());
        let end = (comment.span.end as usize).min(source.len()).max(start);
        let Some(content) = source
            .get(start..end)
            .and_then(|raw| raw.strip_prefix("///"))
        else {
            continue;
        };
        let content = content.trim();
        if !content.starts_with("<reference") {
            continue;
        }
        for token in content.split_whitespace().skip(1) {
            let token = token.trim_end_matches(['/', '>']);
            if let Some(("types", value)) = token.split_once('=') {
                specifiers.insert(value.trim_matches(['"', '\'']).to_owned());
            }
        }
    }

    specifiers.into_iter().collect()
}

/// Parse an ESM module after specializing its directly declared exports to a
/// closed-world demanded set and running Oxc's conservative DCE pass.
///
/// Export forms that cannot be safely converted back into an ordinary local
/// declaration remain observable roots. The caller must relink the returned
/// module because its import and export interface may be smaller.
pub fn parse_dce(path: &str, source: &str, demanded_exports: &BTreeSet<String>) -> Module {
    let mut source_type = SourceType::from_path(Path::new(path))
        .unwrap_or_else(|_| SourceType::default().with_module(true));
    if !path.ends_with(".cjs") {
        source_type = source_type.with_module(true);
    }
    parse_source_in_context(
        Some(path),
        source,
        source_type,
        None,
        Some(demanded_exports),
        None,
    )
}

/// Parse an ESM module for a native binary, where `import.meta` resolves at
/// runtime from `process.execPath` (Node SEA semantics) instead of baking in
/// build-time source paths. `is_entry` controls `import.meta.main`.
pub fn parse_dce_native(
    path: &str,
    source: &str,
    demanded_exports: &BTreeSet<String>,
    is_entry: bool,
) -> Module {
    let mut source_type = SourceType::from_path(Path::new(path))
        .unwrap_or_else(|_| SourceType::default().with_module(true));
    if !path.ends_with(".cjs") {
        source_type = source_type.with_module(true);
    }
    parse_source_in_context(
        Some(path),
        source,
        source_type,
        None,
        Some(demanded_exports),
        Some(is_entry),
    )
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
    parse_source_in_context(
        Some(path),
        source,
        inferred.with_script(true),
        Some(allow),
        None,
        None,
    )
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

/// Discover literal Node loader requests without guessing from identifier spelling.
///
/// Oxc's semantic pass gives every reference its resolved symbol, which is the only reliable way
/// to distinguish an imported `createRequire` from a same-named parameter or local function. The
/// returned facts remain lazy graph edges; this pass never resolves or evaluates them.
pub fn discover_node_runtime_requests(path: &str, source: &str) -> Vec<NodeRuntimeRequest> {
    let allocator = Allocator::default();
    let mut source_type = SourceType::from_path(Path::new(path)).unwrap_or_default();
    if Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        == Some("cjs")
    {
        source_type = source_type.with_commonjs(true);
    }
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions {
            parse_regular_expression: true,
            ..ParseOptions::default()
        })
        .parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return Vec::new();
    }
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program)
        .semantic;
    discover_node_runtime_requests_from_semantic(path, &parsed.program, &semantic)
}

fn discover_node_runtime_requests_from_semantic(
    path: &str,
    program: &oxc_ast::ast::Program<'_>,
    semantic: &oxc_semantic::Semantic<'_>,
) -> Vec<NodeRuntimeRequest> {
    let scoping = semantic.scoping();
    let mut create_symbols = HashSet::<SymbolId>::new();
    let mut module_symbols = HashMap::<SymbolId, String>::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if import.source.value != "node:module" && import.source.value != "module" {
            continue;
        }
        let Some(specifiers) = &import.specifiers else {
            continue;
        };
        for specifier in specifiers {
            let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier else {
                continue;
            };
            let imported = match &specifier.imported {
                oxc_ast::ast::ModuleExportName::IdentifierName(name) => name.name.as_str(),
                oxc_ast::ast::ModuleExportName::IdentifierReference(name) => name.name.as_str(),
                oxc_ast::ast::ModuleExportName::StringLiteral(name) => name.value.as_str(),
            };
            if imported == "createRequire"
                && let Some(symbol) = specifier.local.symbol_id.get()
            {
                create_symbols.insert(symbol);
            }
        }
    }

    fn is_global_require_of_module(
        expression: &Expression<'_>,
        scoping: &oxc_semantic::Scoping,
    ) -> bool {
        let Expression::CallExpression(call) = expression else {
            return false;
        };
        let Expression::Identifier(identifier) = &call.callee else {
            return false;
        };
        if identifier.name != "require" || !identifier.is_global_reference(scoping) {
            return false;
        }
        matches!(
            call.arguments.first().and_then(Argument::as_expression),
            Some(Expression::StringLiteral(literal))
                if literal.value == "node:module" || literal.value == "module"
        )
    }

    fn literal_argument(call: &oxc_ast::ast::CallExpression<'_>) -> Option<String> {
        match call.arguments.first()? {
            Argument::StringLiteral(literal) => Some(literal.value.to_string()),
            // `` `node:os` `` with no interpolations is as static as `"node:os"`.
            Argument::TemplateLiteral(template)
                if template.expressions.is_empty() && template.quasis.len() == 1 =>
            {
                Some(template.quasis[0].value.cooked.as_ref()?.to_string())
            }
            _ => None,
        }
    }

    fn imported_factory_base(
        call: &oxc_ast::ast::CallExpression<'_>,
        create_symbols: &HashSet<SymbolId>,
        module_symbols: &HashMap<SymbolId, String>,
        scoping: &oxc_semantic::Scoping,
        path: &str,
    ) -> Option<String> {
        let base = match &call.callee {
            Expression::Identifier(identifier) => identifier
                .reference_id
                .get()
                .and_then(|reference| scoping.get_reference(reference).symbol_id())
                .and_then(|symbol| create_symbols.contains(&symbol).then(|| path.to_owned())),
            Expression::StaticMemberExpression(member)
                if member.property.name == "createRequire" =>
            {
                let Expression::Identifier(identifier) = &member.object else {
                    return None;
                };
                identifier
                    .reference_id
                    .get()
                    .and_then(|reference| scoping.get_reference(reference).symbol_id())
                    .and_then(|symbol| module_symbols.get(&symbol).cloned())
            }
            _ => None,
        };
        let argument = call.arguments.first().and_then(Argument::as_expression)?;
        if matches!(argument, Expression::StaticMemberExpression(member) if member.property.name == "url" && matches!(member.object, Expression::ImportMeta(_)))
            || matches!(argument, Expression::Identifier(identifier) if identifier.name == "__filename" && identifier.is_global_reference(scoping))
        {
            return base;
        }
        if let Expression::StringLiteral(literal) = argument
            && (literal.value.starts_with('/') || literal.value.starts_with("file:"))
        {
            return Some(literal.value.to_string());
        }
        None
    }

    create_symbols.retain(|symbol| !scoping.symbol_is_mutated(*symbol));
    let mut require_symbols = HashMap::new();
    // First collect CommonJS `const m = require("node:module")` symbols. Do this as a separate
    // pass because semantic node order is an AST walk detail, not a source-order contract.
    for node in semantic.nodes() {
        let AstKind::VariableDeclarator(declarator) = node.kind() else {
            continue;
        };
        let Some(init) = &declarator.init else {
            continue;
        };
        let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
            continue;
        };
        if let Some(symbol) = identifier.symbol_id.get() {
            if !scoping.symbol_is_mutated(symbol) && is_global_require_of_module(init, scoping) {
                module_symbols.insert(symbol, path.to_owned());
            }
        }
    }
    // Track only aliases initialized from the proven imported/module factory. A shadowed local
    // with the same spelling has a different SymbolId and therefore cannot enter this set.
    for node in semantic.nodes() {
        let AstKind::VariableDeclarator(declarator) = node.kind() else {
            continue;
        };
        let Some(init) = &declarator.init else {
            continue;
        };
        let BindingPattern::BindingIdentifier(identifier) = &declarator.id else {
            continue;
        };
        if let Some(symbol) = identifier.symbol_id.get() {
            if scoping.symbol_is_mutated(symbol) {
                continue;
            }
            let Expression::CallExpression(factory) = init else {
                continue;
            };
            if let Some(base) =
                imported_factory_base(factory, &create_symbols, &module_symbols, scoping, path)
            {
                require_symbols.insert(symbol, base);
            }
        }
    }

    let mut requests = Vec::new();
    for node in semantic.nodes() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        if let Expression::Identifier(identifier) = &call.callee {
            let Some(reference) = identifier.reference_id.get() else {
                continue;
            };
            let Some(symbol) = scoping.get_reference(reference).symbol_id() else {
                continue;
            };
            let Some(importer) = require_symbols.get(&symbol) else {
                continue;
            };
            let Some(specifier) = literal_argument(call) else {
                continue;
            };
            requests.push(NodeRuntimeRequest {
                specifier,
                importer: importer.clone(),
            });
            continue;
        } else if let Expression::CallExpression(factory) = &call.callee {
            let Some(importer) =
                imported_factory_base(factory, &create_symbols, &module_symbols, scoping, path)
            else {
                continue;
            };
            let Some(specifier) = literal_argument(call) else {
                continue;
            };
            requests.push(NodeRuntimeRequest {
                specifier,
                importer,
            });
            continue;
        } else if let Expression::StaticMemberExpression(member) = &call.callee {
            // `x.getBuiltinModule(id)`: the receiver is usually the global `process`, but
            // bundlers routinely alias it (`e.getBuiltinModule(...)` with `e` a parameter),
            // so any receiver is accepted. A literal id is a statically-provable builtin
            // request (ADR 0067 §2); a user-defined `getBuiltinModule` merely links one
            // extra builtin, which is harmless.
            if member.property.name != "getBuiltinModule" {
                continue;
            }
            let Some(specifier) = literal_argument(call) else {
                continue;
            };
            requests.push(NodeRuntimeRequest {
                specifier,
                importer: path.to_owned(),
            });
            continue;
        } else {
            continue;
        }
    }
    // A `node:worker_threads` worker runs another module of the program as its entry, so the
    // module a `new Worker(...)` names is a lazy edge like a `createRequire` target: in the graph
    // and live, evaluated only when the worker starts. The spellings that name a module
    // independently of the working directory are recognised -- `new URL("./w.js",
    // import.meta.url)`, `path.join(__dirname, "w.js")`, `fileURLToPath(...)` of either -- and a
    // literal path, which node resolves against the working directory and the graph against
    // the module's (the two agree when a program is run from its own directory).
    for node in semantic.nodes() {
        let AstKind::NewExpression(new) = node.kind() else {
            continue;
        };
        let names_worker = match &new.callee {
            Expression::Identifier(identifier) => identifier.name == "Worker",
            Expression::StaticMemberExpression(member) => member.property.name == "Worker",
            _ => false,
        };
        if !names_worker {
            continue;
        }
        let evaluates_source = new
            .arguments
            .get(1)
            .and_then(Argument::as_expression)
            .is_some_and(|options| match options {
                Expression::ObjectExpression(object) => object.properties.iter().any(|property| {
                    matches!(property, ObjectPropertyKind::ObjectProperty(property)
                        if property.key.static_name().is_some_and(|name| name == "eval")
                            && !matches!(property.value, Expression::BooleanLiteral(ref flag) if !flag.value))
                }),
                _ => false,
            });
        if evaluates_source {
            continue;
        }
        let Some(argument) = new.arguments.first().and_then(Argument::as_expression) else {
            continue;
        };
        let mut specifiers = Vec::new();
        worker_script_specifiers(argument, semantic, 0, &mut specifiers);
        for specifier in specifiers {
            requests.push(NodeRuntimeRequest {
                specifier,
                importer: path.to_owned(),
            });
        }
    }
    // An actor names code in its destination realm with `{ module: "..." }`, rather than
    // handing a constructor across. Retain statically named targets as lazy graph nodes, just
    // as for `new Worker(path)`. Runtime still refuses a computed module absent from the graph.
    for node in semantic.nodes() {
        let AstKind::CallExpression(call) = node.kind() else {
            continue;
        };
        let names_spawn = match &call.callee {
            Expression::Identifier(name) => name.name == "spawnActor",
            Expression::StaticMemberExpression(member) => member.property.name == "spawnActor",
            _ => false,
        };
        if !names_spawn {
            continue;
        }
        let Some(Expression::ObjectExpression(options)) =
            call.arguments.first().and_then(Argument::as_expression)
        else {
            continue;
        };
        for entry in &options.properties {
            let ObjectPropertyKind::ObjectProperty(property) = entry else {
                continue;
            };
            if property
                .key
                .static_name()
                .is_none_or(|name| name != "module")
            {
                continue;
            }
            let mut specifiers = Vec::new();
            worker_script_specifiers(&property.value, semantic, 0, &mut specifiers);
            for specifier in specifiers {
                requests.push(NodeRuntimeRequest {
                    specifier,
                    importer: path.to_owned(),
                });
            }
        }
    }
    requests.sort_by(|left, right| left.specifier.cmp(&right.specifier));
    requests.dedup_by(|left, right| {
        left.specifier == right.specifier && left.importer == right.importer
    });
    requests
}

/// Every module a `new Worker(argument)` can name: through a `const` binding (`const script =
/// join(__dirname, "w.js"); new Worker(script)`) and through each side of a `||`, `??` or
/// conditional (`override || join(__dirname, "lib", "worker.js")`, as thread-stream writes it).
fn worker_script_specifiers(
    argument: &Expression<'_>,
    semantic: &oxc_semantic::Semantic<'_>,
    depth: usize,
    out: &mut Vec<String>,
) {
    if depth > 8 {
        return;
    }
    match argument {
        Expression::ParenthesizedExpression(inner) => {
            worker_script_specifiers(&inner.expression, semantic, depth + 1, out)
        }
        Expression::LogicalExpression(logical) => {
            worker_script_specifiers(&logical.left, semantic, depth + 1, out);
            worker_script_specifiers(&logical.right, semantic, depth + 1, out);
        }
        Expression::ConditionalExpression(conditional) => {
            worker_script_specifiers(&conditional.consequent, semantic, depth + 1, out);
            worker_script_specifiers(&conditional.alternate, semantic, depth + 1, out);
        }
        Expression::Identifier(identifier) => {
            let scoping = semantic.scoping();
            let Some(symbol) = identifier
                .reference_id
                .get()
                .and_then(|reference| scoping.get_reference(reference).symbol_id())
            else {
                return;
            };
            if scoping.symbol_is_mutated(symbol) {
                return;
            }
            let declaration = semantic
                .nodes()
                .get_node(scoping.symbol_declaration(symbol));
            if let AstKind::VariableDeclarator(declarator) = declaration.kind()
                && let Some(init) = &declarator.init
            {
                worker_script_specifiers(init, semantic, depth + 1, out);
            }
        }
        other => {
            if let Some(specifier) = worker_script_specifier(other, semantic.scoping()) {
                out.push(specifier);
            }
        }
    }
}

/// The module a `new Worker(argument)` names, as a specifier relative to the module that says
/// it, when the argument spells one without depending on run-time values.
fn worker_script_specifier(
    argument: &Expression<'_>,
    scoping: &oxc_semantic::Scoping,
) -> Option<String> {
    fn text(expression: &Expression<'_>) -> Option<String> {
        match expression {
            Expression::StringLiteral(literal) => Some(literal.value.to_string()),
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => template
                .quasis
                .first()
                .and_then(|quasi| quasi.value.cooked.as_ref())
                .map(|cooked| cooked.to_string()),
            _ => None,
        }
    }
    fn is_import_meta_url(expression: &Expression<'_>) -> bool {
        matches!(expression, Expression::StaticMemberExpression(member)
            if member.property.name == "url" && matches!(member.object, Expression::ImportMeta(_)))
    }
    match argument {
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => {
            let path = text(argument)?;
            (path.starts_with("./") || path.starts_with("../") || path.starts_with('/'))
                .then_some(path)
        }
        Expression::NewExpression(url) => {
            let Expression::Identifier(callee) = &url.callee else {
                return None;
            };
            if callee.name != "URL" {
                return None;
            }
            let relative = text(url.arguments.first()?.as_expression()?)?;
            if !is_import_meta_url(url.arguments.get(1)?.as_expression()?) {
                return None;
            }
            Some(
                if relative.starts_with("./") || relative.starts_with("../") {
                    relative
                } else {
                    format!("./{relative}")
                },
            )
        }
        Expression::CallExpression(call) => {
            // A compiler's `(0, path_1.resolve)(...)` calls the member without a receiver.
            let mut callee = &call.callee;
            loop {
                callee = match callee {
                    Expression::SequenceExpression(sequence) => sequence.expressions.last()?,
                    Expression::ParenthesizedExpression(inner) => &inner.expression,
                    _ => break,
                };
            }
            let callee = match callee {
                Expression::Identifier(identifier) => identifier.name.as_str(),
                Expression::StaticMemberExpression(member) => member.property.name.as_str(),
                _ => return None,
            };
            match callee {
                "fileURLToPath" => {
                    worker_script_specifier(call.arguments.first()?.as_expression()?, scoping)
                }
                "join" | "resolve" => {
                    let mut arguments = call.arguments.iter();
                    let Expression::Identifier(base) = arguments.next()?.as_expression()? else {
                        return None;
                    };
                    if base.name != "__dirname" || !base.is_global_reference(scoping) {
                        return None;
                    }
                    let mut parts = Vec::new();
                    for argument in arguments {
                        parts.push(text(argument.as_expression()?)?);
                    }
                    let joined = parts.join("/");
                    Some(if joined.starts_with("./") || joined.starts_with("../") {
                        joined
                    } else {
                        format!("./{joined}")
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
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
    let mut type_syntax = capture_type_syntax(path, source, &parsed.program, None);
    if source_type.is_typescript() {
        type_syntax.body = capture_original_body(path, source, &parsed, None);
        retain_erased_declarations(&mut type_syntax);
    }

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

    let (requires, node_runtime_requests) = if parsed.panicked || !parsed.diagnostics.is_empty() {
        (Vec::new(), Vec::new())
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
        (
            requires
                .into_iter()
                .filter_map(|(_, value)| seen.insert(value.clone()).then_some(value))
                .collect(),
            discover_node_runtime_requests_from_semantic(
                path.unwrap_or("inline.cjs"),
                &parsed.program,
                &semantic,
            ),
        )
    };
    let mut module = lower_parsed(path, &parsed, source, source_type);
    module.node_runtime_requests = node_runtime_requests;
    module.type_syntax = type_syntax;
    (module, requires)
}

/// Stop undemanded direct declarations from being implicit roots in Oxc's
/// module liveness. The declaration remains in place as an ordinary local, so
/// Oxc still preserves its initializer whenever that initializer can have an
/// observable effect.
fn expose_only_demanded_declarations<'a>(
    allocator: &'a Allocator,
    program: &mut oxc_ast::ast::Program<'a>,
    demanded_exports: &BTreeSet<String>,
) {
    for statement in &mut program.body {
        match statement {
            Statement::ExportNamedDeclaration(export) => {
                export.specifiers.retain(|specifier| {
                    demanded_exports.contains(specifier.exported.name().as_str())
                });
            }
            Statement::ExportFromDeclaration(export) => {
                export.specifiers.retain(|specifier| {
                    demanded_exports.contains(specifier.exported.name().as_str())
                });
            }
            _ => {}
        }
        let should_deexport = match statement {
            Statement::ExportDeclaration(export) => {
                // Oxc 0.147 may erase an unused inherited class even though evaluating its
                // heritage can throw. Keeping just that declaration exported makes it a DCE root
                // while still allowing every unrelated declaration in the module to be shaken.
                if declaration_has_class_heritage(&export.declaration) {
                    continue;
                }
                let mut demanded = false;
                export.declaration.bound_names(&mut |identifier| {
                    demanded |= demanded_exports.contains(identifier.name.as_str());
                });
                !demanded
            }
            _ => false,
        };
        if !should_deexport {
            continue;
        }
        let Statement::ExportDeclaration(export) = statement.take_in(&allocator) else {
            unreachable!("checked export declaration above")
        };
        *statement = export.unbox().declaration.into();
    }
}

fn variable_declaration_has_class_heritage(declaration: &VariableDeclaration<'_>) -> bool {
    declaration.declarations.iter().any(|declarator| {
        matches!(
            declarator.init.as_ref(),
            Some(Expression::ClassExpression(class)) if class.heritage.is_some()
        )
    })
}

fn declaration_has_class_heritage(declaration: &Declaration<'_>) -> bool {
    match declaration {
        Declaration::ClassDeclaration(class) => class.heritage.is_some(),
        Declaration::VariableDeclaration(declaration) => {
            variable_declaration_has_class_heritage(declaration)
        }
        _ => false,
    }
}

/// Oxc 0.147 can treat an unused inherited class as removable even though
/// evaluating its heritage may throw (notably `class A extends Later {}`).
/// Exported declarations can remain roots while their neighbors are shaken. An unexported one
/// has no equivalent root in the compressor API, so only that narrower case keeps the module on
/// the conservative path.
fn has_unexported_top_level_class_heritage(program: &oxc_ast::ast::Program<'_>) -> bool {
    program.body.iter().any(|statement| match statement {
        Statement::ClassDeclaration(class) => class.heritage.is_some(),
        Statement::VariableDeclaration(declaration) => {
            variable_declaration_has_class_heritage(declaration)
        }
        _ => false,
    })
}

fn parse_source(path: Option<&str>, source: &str, source_type: SourceType) -> Module {
    parse_source_in_context(path, source, source_type, None, None, None)
}

fn parse_source_in_context(
    path: Option<&str>,
    source: &str,
    source_type: SourceType,
    allow: Option<&EvalSyntax>,
    demanded_exports: Option<&BTreeSet<String>>,
    native_import_meta: Option<bool>,
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
    // Type syntax must come from the original AST, before JSX lowering and DCE mutate it.
    let mut type_syntax = capture_type_syntax(path, source, &parsed.program, native_import_meta);
    if source_type.is_typescript() {
        type_syntax.body = capture_original_body(path, source, &parsed, native_import_meta);
        retain_erased_declarations(&mut type_syntax);
    }

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

    if let Some(demanded_exports) = demanded_exports
        && !parsed.panicked
        && parsed.diagnostics.is_empty()
        && early_errors(&parsed).is_empty()
        && !has_unexported_top_level_class_heritage(&parsed.program)
    {
        expose_only_demanded_declarations(&allocator, &mut parsed.program, demanded_exports);
        // Oxc's side-effect model assumes coercions are pure and that some global functions
        // never throw; guard what the language can observe before it runs (issue 19).
        dce_guard::guard_observable_effects(&allocator, &mut parsed.program);
        Compressor::new(&allocator)
            .dead_code_elimination(&mut parsed.program, CompressOptions::dce());
        dce_guard::release_observable_guards(&allocator, &mut parsed.program);
    }
    let node_runtime_requests = if parsed.panicked || !parsed.diagnostics.is_empty() {
        Vec::new()
    } else {
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program)
            .semantic;
        discover_node_runtime_requests_from_semantic(
            path.unwrap_or("inline.js"),
            &parsed.program,
            &semantic,
        )
    };
    let mut module = lower_parsed_in_context(
        path,
        &parsed,
        source,
        source_type,
        allow,
        demanded_exports,
        native_import_meta,
    );
    module.node_runtime_requests = node_runtime_requests;
    module.type_syntax = type_syntax;
    module
}

fn module_attribute_keyword_span(
    source: &str,
    statement: Span,
    clause_start: u32,
    keyword: &str,
) -> Option<HirSpan> {
    fn identifier_continue(ch: char) -> bool {
        ch == '_' || ch == '$' || ch.is_alphanumeric() || (!ch.is_ascii() && !ch.is_whitespace())
    }
    fn trivia_ends_at(source: &str, mut cursor: usize, end: usize) -> bool {
        let bytes = source.as_bytes();
        while cursor < end {
            if bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            } else if bytes
                .get(cursor..cursor + 2)
                .is_some_and(|token| token == b"/*")
            {
                let Some(relative_end) = source[cursor + 2..end].find("*/") else {
                    return false;
                };
                cursor += relative_end + 4;
            } else if bytes
                .get(cursor..cursor + 2)
                .is_some_and(|token| token == b"//")
            {
                cursor += 2;
                while cursor < end && !matches!(bytes[cursor], b'\n' | b'\r') {
                    cursor += 1;
                }
            } else {
                return false;
            }
        }
        cursor == end
    }

    let start = (statement.start as usize).min(source.len());
    let end = (clause_start as usize).min(source.len());
    let slice = source.get(start..end)?;
    for (relative, _) in slice.rmatch_indices(keyword) {
        let candidate = start + relative;
        let after = candidate + keyword.len();
        let before_boundary = source[..candidate]
            .chars()
            .next_back()
            .is_none_or(|ch| !identifier_continue(ch));
        let after_boundary = source[after..]
            .chars()
            .next()
            .is_none_or(|ch| !identifier_continue(ch));
        if before_boundary && after_boundary && trivia_ends_at(source, after, end) {
            return Some(HirSpan::new(candidate as u32, after as u32));
        }
    }
    None
}

fn module_attribute_record(
    source: &str,
    statement_span: Span,
    with_clause: &oxc_ast::ast::WithClause<'_>,
) -> kiln_hir::TypeModuleAttributes {
    use oxc_ast::ast::{ImportAttributeKey, WithClauseKeyword};
    let keyword = match with_clause.keyword {
        WithClauseKeyword::With => kiln_hir::TypeModuleAttributesKeyword::With,
        WithClauseKeyword::Assert => kiln_hir::TypeModuleAttributesKeyword::Assert,
    };
    let keyword_len = match keyword {
        kiln_hir::TypeModuleAttributesKeyword::With => 4,
        kiln_hir::TypeModuleAttributesKeyword::Assert => 6,
    };
    let keyword_text = match keyword {
        kiln_hir::TypeModuleAttributesKeyword::With => "with",
        kiln_hir::TypeModuleAttributesKeyword::Assert => "assert",
    };
    let keyword_span =
        module_attribute_keyword_span(source, statement_span, with_clause.span.start, keyword_text)
            .unwrap_or_else(|| {
                HirSpan::new(with_clause.span.start, with_clause.span.start + keyword_len)
            });
    kiln_hir::TypeModuleAttributes {
        statement_span: HirSpan::new(statement_span.start, statement_span.end),
        keyword_span,
        attributes_span: HirSpan::new(with_clause.span.start, with_clause.span.end),
        keyword,
        attributes: with_clause
            .with_entries
            .iter()
            .map(|attribute| {
                let (key, key_kind, key_span) = match &attribute.key {
                    ImportAttributeKey::Identifier(identifier) => (
                        StringUnits::from(identifier.name.as_str()),
                        kiln_hir::TypeModuleAttributeKeyKind::Identifier,
                        identifier.span,
                    ),
                    ImportAttributeKey::StringLiteral(literal) => (
                        oxc_string_units(literal.value.as_str(), literal.lone_surrogates),
                        kiln_hir::TypeModuleAttributeKeyKind::StringLiteral,
                        literal.span,
                    ),
                };
                kiln_hir::TypeModuleAttribute {
                    key,
                    key_kind,
                    value: oxc_string_units(
                        attribute.value.value.as_str(),
                        attribute.value.lone_surrogates,
                    ),
                    span: HirSpan::new(attribute.span.start, attribute.span.end),
                    key_span: HirSpan::new(key_span.start, key_span.end),
                    value_span: HirSpan::new(attribute.value.span.start, attribute.value.span.end),
                }
            })
            .collect(),
    }
}

fn capture_module_attributes(
    source: &str,
    program: &oxc_ast::ast::Program<'_>,
) -> Vec<kiln_hir::TypeModuleAttributes> {
    struct Collector<'s> {
        source: &'s str,
        attributes: Vec<kiln_hir::TypeModuleAttributes>,
    }
    impl<'a> Visit<'a> for Collector<'_> {
        fn visit_import_declaration(&mut self, node: &oxc_ast::ast::ImportDeclaration<'a>) {
            if let Some(with_clause) = node.with_clause.as_deref() {
                self.attributes
                    .push(module_attribute_record(self.source, node.span, with_clause));
            }
            walk::walk_import_declaration(self, node);
        }

        fn visit_export_from_declaration(
            &mut self,
            node: &oxc_ast::ast::ExportFromDeclaration<'a>,
        ) {
            if let Some(with_clause) = node.with_clause.as_deref() {
                self.attributes
                    .push(module_attribute_record(self.source, node.span, with_clause));
            }
            walk::walk_export_from_declaration(self, node);
        }

        fn visit_export_all_declaration(&mut self, node: &oxc_ast::ast::ExportAllDeclaration<'a>) {
            if let Some(with_clause) = node.with_clause.as_deref() {
                self.attributes
                    .push(module_attribute_record(self.source, node.span, with_clause));
            }
            walk::walk_export_all_declaration(self, node);
        }
    }
    let mut collector = Collector {
        source,
        attributes: Vec::new(),
    };
    collector.visit_program(program);
    collector
        .attributes
        .sort_by_key(|attributes| attributes.statement_span.start);
    collector.attributes
}

/// Copy source-level TS declarations and annotation expressions out of Oxc's arena before any
/// transformer or minifier can erase them. Raw type text is retained even when this adapter has
/// not yet gained a structured lowering for a particular Oxc type node.
fn capture_type_syntax(
    path: Option<&str>,
    source: &str,
    program: &oxc_ast::ast::Program<'_>,
    native_import_meta: Option<bool>,
) -> HirTypeSyntax {
    let module_attributes = capture_module_attributes(source, program);
    if !program.source_type.is_typescript() {
        return HirTypeSyntax {
            module_attributes,
            ..HirTypeSyntax::default()
        };
    }
    struct Collector<'s> {
        source: &'s str,
        path: Option<String>,
        native_import_meta: Option<bool>,
        result: HirTypeSyntax,
        classified: HashSet<(u32, u32)>,
    }
    impl Collector<'_> {
        fn is_exported(&self, span: oxc_span::Span) -> bool {
            self.result
                .exported_declarations
                .iter()
                .any(|declaration| declaration.start == span.start && declaration.end == span.end)
        }

        fn is_default_exported(&self, span: oxc_span::Span) -> bool {
            self.result
                .default_exported_declarations
                .iter()
                .any(|declaration| declaration.start == span.start && declaration.end == span.end)
        }

        fn raw(&self, span: Span) -> String {
            let start = (span.start as usize).min(self.source.len());
            let end = (span.end as usize).min(self.source.len()).max(start);
            self.source.get(start..end).unwrap_or("").to_owned()
        }
        fn owned_key(
            &self,
            key: &oxc_ast::ast::PropertyKey<'_>,
            computed: bool,
        ) -> Option<kiln_hir::ObjectKey> {
            let mut lowerer = Lowerer {
                text: self.source,
                path: self.path.as_deref(),
                native_import_meta: self.native_import_meta,
            };
            lowerer.member_key(key, computed)
        }
        fn type_name_parts(&self, name: &oxc_ast::ast::TSTypeName<'_>) -> Vec<String> {
            match name {
                oxc_ast::ast::TSTypeName::IdentifierReference(identifier) => {
                    vec![identifier.name.to_string()]
                }
                oxc_ast::ast::TSTypeName::QualifiedName(qualified) => {
                    self.qualified_type_name_parts(qualified)
                }
                oxc_ast::ast::TSTypeName::ThisExpression(_) => vec!["this".to_owned()],
            }
        }
        fn query_name_parts(&self, name: &oxc_ast::ast::TSTypeQueryExprName<'_>) -> Vec<String> {
            match name {
                oxc_ast::ast::TSTypeQueryExprName::IdentifierReference(identifier) => {
                    vec![identifier.name.to_string()]
                }
                oxc_ast::ast::TSTypeQueryExprName::QualifiedName(qualified) => {
                    self.qualified_type_name_parts(qualified)
                }
                oxc_ast::ast::TSTypeQueryExprName::ThisExpression(_) => vec!["this".to_owned()],
                oxc_ast::ast::TSTypeQueryExprName::TSImportType(_) => Vec::new(),
            }
        }
        fn qualified_type_name_parts(
            &self,
            name: &oxc_ast::ast::TSQualifiedName<'_>,
        ) -> Vec<String> {
            let mut parts = self.type_name_parts(&name.left);
            parts.push(name.right.name.to_string());
            parts
        }
        fn import_qualifier_parts(
            &self,
            qualifier: &oxc_ast::ast::TSImportTypeQualifier<'_>,
        ) -> Vec<String> {
            match qualifier {
                oxc_ast::ast::TSImportTypeQualifier::Identifier(identifier) => {
                    vec![identifier.name.to_string()]
                }
                oxc_ast::ast::TSImportTypeQualifier::QualifiedName(qualified) => {
                    let mut parts = self.import_qualifier_parts(&qualified.left);
                    parts.push(qualified.right.name.to_string());
                    parts
                }
            }
        }
        fn heritage_name_parts(
            &self,
            expression: &oxc_ast::ast::Expression<'_>,
        ) -> Option<Vec<String>> {
            match expression {
                oxc_ast::ast::Expression::Identifier(identifier) => {
                    Some(vec![identifier.name.to_string()])
                }
                oxc_ast::ast::Expression::StaticMemberExpression(member) => {
                    let mut parts = self.heritage_name_parts(&member.object)?;
                    parts.push(member.property.name.to_string());
                    Some(parts)
                }
                oxc_ast::ast::Expression::ParenthesizedExpression(parenthesized) => {
                    self.heritage_name_parts(&parenthesized.expression)
                }
                _ => None,
            }
        }
        fn import_type_options(
            &self,
            options: Option<&oxc_ast::ast::ObjectExpression<'_>>,
        ) -> Option<Box<kiln_hir::Expression>> {
            let options = options?;
            let mut lowerer = Lowerer {
                text: self.source,
                path: self.path.as_deref(),
                native_import_meta: self.native_import_meta,
            };
            Some(Box::new(lowerer.object_literal(
                options,
                HirSpan::new(options.span.start, options.span.end),
            )))
        }
        fn operand_span(&self, expression: &oxc_ast::ast::Expression<'_>) -> HirSpan {
            let mut expression = expression;
            while let oxc_ast::ast::Expression::ParenthesizedExpression(parenthesized) = expression
            {
                expression = &parenthesized.expression;
            }
            HirSpan::new(expression.span().start, expression.span().end)
        }
        fn accessibility(
            value: Option<oxc_ast::ast::TSAccessibility>,
        ) -> Option<kiln_hir::TypeAccessibility> {
            value.map(|value| match value {
                oxc_ast::ast::TSAccessibility::Public => kiln_hir::TypeAccessibility::Public,
                oxc_ast::ast::TSAccessibility::Protected => kiln_hir::TypeAccessibility::Protected,
                oxc_ast::ast::TSAccessibility::Private => kiln_hir::TypeAccessibility::Private,
            })
        }
        fn this_parameter(
            &self,
            this_parameter: Option<&oxc_ast::ast::TSThisParameter<'_>>,
        ) -> Option<kiln_hir::TypeSignatureParameter> {
            this_parameter.map(|parameter| kiln_hir::TypeSignatureParameter {
                name: Some("this".to_owned()),
                pattern: Some(Box::new(kiln_hir::Pattern::Identifier("this".to_owned()))),
                pattern_span: HirSpan::new(parameter.this_span.start, parameter.this_span.end),
                span: HirSpan::new(parameter.span.start, parameter.span.end),
                ty: parameter
                    .type_annotation
                    .as_deref()
                    .map(|annotation| self.convert(&annotation.type_annotation)),
                optional: false,
                rest: false,
                has_default: false,
            })
        }
        fn owned_pattern(
            &self,
            pattern: &oxc_ast::ast::BindingPattern<'_>,
            span: Span,
        ) -> kiln_hir::Pattern {
            let mut lowerer = Lowerer {
                text: self.source,
                path: self.path.as_deref(),
                native_import_meta: self.native_import_meta,
            };
            lowerer.pattern(pattern, HirSpan::new(span.start, span.end))
        }
        fn convert(&self, ty: &oxc_ast::ast::TSType<'_>) -> HirTypeExpr {
            use oxc_ast::ast::TSType as T;
            let span = ty.span();
            let hir_span = HirSpan::new(span.start, span.end);
            let kind = match ty {
                T::TSAnyKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Any),
                T::TSUnknownKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Unknown),
                T::TSNeverKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Never),
                T::TSVoidKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Void),
                T::TSUndefinedKeyword(_) => {
                    HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Undefined)
                }
                T::TSNullKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Null),
                T::TSBooleanKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Boolean),
                T::TSNumberKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Number),
                T::TSBigIntKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::BigInt),
                T::TSStringKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::String),
                T::TSSymbolKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Symbol),
                T::TSObjectKeyword(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Object),
                T::TSThisType(_) => HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::This),
                T::TSIntrinsicKeyword(_) => {
                    HirTypeExprKind::Keyword(kiln_hir::TypeKeyword::Intrinsic)
                }
                T::TSUnionType(n) => {
                    HirTypeExprKind::Union(n.types.iter().map(|t| self.convert(t)).collect())
                }
                T::TSIntersectionType(n) => {
                    HirTypeExprKind::Intersection(n.types.iter().map(|t| self.convert(t)).collect())
                }
                T::TSArrayType(n) => {
                    HirTypeExprKind::Array(Box::new(self.convert(&n.element_type)))
                }
                T::TSTypeLiteral(n) => HirTypeExprKind::Object(self.object_members(&n.members)),
                T::TSMappedType(n) => {
                    let key_span = n.key.span;
                    let modifier =
                        |value: Option<oxc_ast::ast::TSMappedTypeModifierOperator>| match value {
                            None => kiln_hir::TypeModifier::None,
                            Some(
                                oxc_ast::ast::TSMappedTypeModifierOperator::True
                                | oxc_ast::ast::TSMappedTypeModifierOperator::Plus,
                            ) => kiln_hir::TypeModifier::Add,
                            Some(oxc_ast::ast::TSMappedTypeModifierOperator::Minus) => {
                                kiln_hir::TypeModifier::Remove
                            }
                        };
                    HirTypeExprKind::Mapped {
                        parameter: kiln_hir::TypeParameter {
                            name: n.key.name.to_string(),
                            span: HirSpan::new(key_span.start, key_span.end),
                            constraint: None,
                            default: None,
                            const_: false,
                            in_: false,
                            out: false,
                        },
                        constraint: Box::new(self.convert(&n.constraint)),
                        name_type: n.name_type.as_ref().map(|t| Box::new(self.convert(t))),
                        value: Box::new(
                            n.type_annotation
                                .as_ref()
                                .map(|t| self.convert(t))
                                .unwrap_or_else(|| HirTypeExpr {
                                    span: hir_span,
                                    kind: HirTypeExprKind::Unsupported {
                                        syntax: self.raw(span),
                                    },
                                }),
                        ),
                        readonly: modifier(n.readonly),
                        optional: modifier(n.optional),
                    }
                }
                T::TSIndexedAccessType(n) => HirTypeExprKind::IndexedAccess {
                    object: Box::new(self.convert(&n.object_type)),
                    index: Box::new(self.convert(&n.index_type)),
                },
                T::TSParenthesizedType(n) => {
                    HirTypeExprKind::Parenthesized(Box::new(self.convert(&n.type_annotation)))
                }
                T::TSConditionalType(n) => HirTypeExprKind::Conditional {
                    check: Box::new(self.convert(&n.check_type)),
                    extends: Box::new(self.convert(&n.extends_type)),
                    when_true: Box::new(self.convert(&n.true_type)),
                    when_false: Box::new(self.convert(&n.false_type)),
                },
                T::TSFunctionType(n) => {
                    let mut signature = self.signature(
                        n.span,
                        n.type_parameters.as_deref(),
                        &n.params,
                        &n.return_type,
                    );
                    signature.this_parameter =
                        self.this_parameter(n.this_param.as_deref()).map(Box::new);
                    HirTypeExprKind::Function(signature)
                }
                T::TSConstructorType(n) => HirTypeExprKind::Constructor(self.signature(
                    n.span,
                    n.type_parameters.as_deref(),
                    &n.params,
                    &n.return_type,
                )),
                T::TSTypeReference(n) => {
                    let name = self.type_name_parts(&n.type_name);
                    HirTypeExprKind::Reference {
                        name,
                        arguments: n
                            .type_arguments
                            .as_ref()
                            .map(|a| a.params.iter().map(|t| self.convert(t)).collect())
                            .unwrap_or_default(),
                    }
                }
                T::TSLiteralType(n) => {
                    use oxc_ast::ast::TSLiteral;
                    let literal = match &n.literal {
                        TSLiteral::StringLiteral(v) => kiln_hir::TypeLiteral::String(
                            oxc_string_units(v.value.as_str(), v.lone_surrogates),
                        ),
                        TSLiteral::NumericLiteral(v) => kiln_hir::TypeLiteral::Number(
                            kiln_host_abi::number_to_js_string(v.value),
                        ),
                        TSLiteral::BigIntLiteral(v) => {
                            kiln_hir::TypeLiteral::BigInt(v.value.to_string())
                        }
                        TSLiteral::BooleanLiteral(v) => kiln_hir::TypeLiteral::Boolean(v.value),
                        TSLiteral::UnaryExpression(v)
                            if matches!(
                                v.operator,
                                UnaryOperator::UnaryPlus | UnaryOperator::UnaryNegation
                            ) =>
                        {
                            match &v.argument {
                                oxc_ast::ast::Expression::NumericLiteral(n) => {
                                    let value = if v.operator == UnaryOperator::UnaryNegation {
                                        -n.value
                                    } else {
                                        n.value
                                    };
                                    kiln_hir::TypeLiteral::Number(
                                        kiln_host_abi::number_to_js_string(value),
                                    )
                                }
                                oxc_ast::ast::Expression::BigIntLiteral(n) => {
                                    let mut value = n.value.to_string();
                                    if v.operator == UnaryOperator::UnaryNegation {
                                        value.insert(0, '-');
                                    }
                                    kiln_hir::TypeLiteral::BigInt(value)
                                }
                                _ => {
                                    return HirTypeExpr {
                                        span: HirSpan::new(n.span.start, n.span.end),
                                        kind: HirTypeExprKind::Unsupported {
                                            syntax: self.raw(n.span),
                                        },
                                    };
                                }
                            }
                        }
                        TSLiteral::UnaryExpression(_) => {
                            return HirTypeExpr {
                                span: HirSpan::new(n.span.start, n.span.end),
                                kind: HirTypeExprKind::Unsupported {
                                    syntax: self.raw(n.span),
                                },
                            };
                        }
                        TSLiteral::TemplateLiteral(v) => {
                            kiln_hir::TypeLiteral::String(oxc_string_units(
                                v.quasis
                                    .first()
                                    .map(|q| {
                                        q.value
                                            .cooked
                                            .as_ref()
                                            .map_or(q.value.raw.as_str(), |x| x.as_str())
                                    })
                                    .unwrap_or(""),
                                v.quasis.first().is_some_and(|q| q.lone_surrogates),
                            ))
                        }
                    };
                    HirTypeExprKind::Literal(literal)
                }
                T::TSTupleType(n) => HirTypeExprKind::Tuple(
                    n.element_types
                        .iter()
                        .map(|el| {
                            use oxc_ast::ast::TSTupleElement as E;
                            let (el, name, optional) = if let E::TSNamedTupleMember(x) = el {
                                (&x.element_type, Some(x.label.name.to_string()), x.optional)
                            } else {
                                (el, None, false)
                            };
                            let (ty, optional, rest) = match el {
                                E::TSOptionalType(x) => (&x.type_annotation, true, false),
                                E::TSRestType(x) => (&x.type_annotation, optional, true),
                                other => (
                                    other.as_ts_type().expect("TSTupleElement type"),
                                    optional,
                                    false,
                                ),
                            };
                            kiln_hir::TypeTupleElement {
                                name,
                                ty: self.convert(ty),
                                optional,
                                rest,
                            }
                        })
                        .collect(),
                ),
                T::TSTemplateLiteralType(n) => HirTypeExprKind::Template {
                    quasis: n
                        .quasis
                        .iter()
                        .map(|q| {
                            let cooked = q
                                .value
                                .cooked
                                .as_ref()
                                .map_or(q.value.raw.as_str(), |s| s.as_str());
                            oxc_string_units(cooked, q.lone_surrogates)
                        })
                        .collect(),
                    types: n.types.iter().map(|t| self.convert(t)).collect(),
                },
                T::TSTypeQuery(n) => {
                    if let oxc_ast::ast::TSTypeQueryExprName::TSImportType(import) = &n.expr_name {
                        HirTypeExprKind::Import {
                            specifier: import.source.value.to_string(),
                            options: self.import_type_options(import.options.as_deref()),
                            qualifier: import
                                .qualifier
                                .as_ref()
                                .map(|q| self.import_qualifier_parts(q))
                                .unwrap_or_default(),
                            arguments: import
                                .type_arguments
                                .as_ref()
                                .map(|a| a.params.iter().map(|t| self.convert(t)).collect())
                                .unwrap_or_default(),
                            is_typeof: true,
                        }
                    } else {
                        HirTypeExprKind::Typeof {
                            name: self.query_name_parts(&n.expr_name),
                            arguments: n
                                .type_arguments
                                .as_ref()
                                .map(|a| a.params.iter().map(|t| self.convert(t)).collect())
                                .unwrap_or_default(),
                        }
                    }
                }
                T::TSImportType(n) => HirTypeExprKind::Import {
                    specifier: n.source.value.to_string(),
                    options: self.import_type_options(n.options.as_deref()),
                    qualifier: n
                        .qualifier
                        .as_ref()
                        .map(|q| self.import_qualifier_parts(q))
                        .unwrap_or_default(),
                    arguments: n
                        .type_arguments
                        .as_ref()
                        .map(|a| a.params.iter().map(|t| self.convert(t)).collect())
                        .unwrap_or_default(),
                    is_typeof: false,
                },
                T::TSInferType(n) => HirTypeExprKind::Infer(kiln_hir::TypeParameter {
                    name: n.type_parameter.name.name.to_string(),
                    span: HirSpan::new(n.type_parameter.span.start, n.type_parameter.span.end),
                    constraint: n
                        .type_parameter
                        .constraint
                        .as_ref()
                        .map(|t| Box::new(self.convert(t))),
                    default: n
                        .type_parameter
                        .default
                        .as_ref()
                        .map(|t| Box::new(self.convert(t))),
                    const_: n.type_parameter.r#const,
                    in_: n.type_parameter.r#in,
                    out: n.type_parameter.out,
                }),
                T::TSTypePredicate(n) => HirTypeExprKind::Predicate {
                    parameter: match &n.parameter_name {
                        oxc_ast::ast::TSTypePredicateName::Identifier(identifier) => {
                            identifier.name.to_string()
                        }
                        oxc_ast::ast::TSTypePredicateName::This(_) => "this".to_owned(),
                    },
                    asserts: n.asserts,
                    ty: n
                        .type_annotation
                        .as_deref()
                        .map(|a| Box::new(self.convert(&a.type_annotation))),
                },
                T::TSTypeOperatorType(n) => {
                    let op = match n.operator {
                        oxc_ast::ast::TSTypeOperatorOperator::Keyof => {
                            kiln_hir::TypeOperator::Keyof
                        }
                        oxc_ast::ast::TSTypeOperatorOperator::Readonly => {
                            kiln_hir::TypeOperator::Readonly
                        }
                        oxc_ast::ast::TSTypeOperatorOperator::Unique => {
                            kiln_hir::TypeOperator::Unique
                        }
                    };
                    if op == kiln_hir::TypeOperator::Keyof {
                        HirTypeExprKind::Keyof(Box::new(self.convert(&n.type_annotation)))
                    } else {
                        HirTypeExprKind::Operator {
                            operator: op,
                            operand: Box::new(self.convert(&n.type_annotation)),
                        }
                    }
                }
                // Keep exact source for forms whose AST needs a dedicated lowering. Explicit
                // unsupported nodes retain their spelling and span instead of pretending Any.
                _ => HirTypeExprKind::Unsupported {
                    syntax: self.raw(span),
                },
            };
            HirTypeExpr {
                span: hir_span,
                kind,
            }
        }
        fn parameters(
            &self,
            params: Option<&oxc_ast::ast::TSTypeParameterDeclaration<'_>>,
        ) -> Vec<kiln_hir::TypeParameter> {
            params
                .map(|decl| {
                    decl.params
                        .iter()
                        .map(|p| kiln_hir::TypeParameter {
                            name: p.name.name.to_string(),
                            span: HirSpan::new(p.span.start, p.span.end),
                            constraint: p.constraint.as_ref().map(|t| Box::new(self.convert(t))),
                            default: p.default.as_ref().map(|t| Box::new(self.convert(t))),
                            const_: p.r#const,
                            in_: p.r#in,
                            out: p.out,
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        fn signature(
            &self,
            span: Span,
            type_parameters: Option<&oxc_ast::ast::TSTypeParameterDeclaration<'_>>,
            params: &oxc_ast::ast::FormalParameters<'_>,
            return_type: &oxc_ast::ast::TSTypeAnnotation<'_>,
        ) -> kiln_hir::TypeSignature {
            self.signature_with_return(
                span,
                type_parameters,
                params,
                Some(self.convert(&return_type.type_annotation)),
            )
        }
        fn signature_with_return(
            &self,
            span: Span,
            type_parameters: Option<&oxc_ast::ast::TSTypeParameterDeclaration<'_>>,
            params: &oxc_ast::ast::FormalParameters<'_>,
            return_type: Option<HirTypeExpr>,
        ) -> kiln_hir::TypeSignature {
            let mut signature_params = params
                .items
                .iter()
                .map(|p| {
                    let ty = p
                        .type_annotation
                        .as_deref()
                        .map(|a| self.convert(&a.type_annotation));
                    kiln_hir::TypeSignatureParameter {
                        name: match &p.pattern {
                            oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                                Some(id.name.to_string())
                            }
                            _ => None,
                        },
                        pattern: Some(Box::new(self.owned_pattern(&p.pattern, p.pattern.span()))),
                        pattern_span: HirSpan::new(p.pattern.span().start, p.pattern.span().end),
                        span: HirSpan::new(p.span.start, p.span.end),
                        ty,
                        optional: p.optional,
                        rest: false,
                        has_default: p.initializer.is_some(),
                    }
                })
                .collect::<Vec<_>>();
            if let Some(rest) = params.rest.as_deref() {
                let ty = rest
                    .type_annotation
                    .as_deref()
                    .map(|a| self.convert(&a.type_annotation));
                signature_params.push(kiln_hir::TypeSignatureParameter {
                    name: match &rest.rest.argument {
                        oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                            Some(id.name.to_string())
                        }
                        _ => None,
                    },
                    pattern: Some(Box::new(
                        self.owned_pattern(&rest.rest.argument, rest.rest.argument.span()),
                    )),
                    pattern_span: HirSpan::new(
                        rest.rest.argument.span().start,
                        rest.rest.argument.span().end,
                    ),
                    span: HirSpan::new(rest.span.start, rest.span.end),
                    ty,
                    optional: false,
                    rest: true,
                    has_default: false,
                });
            }
            kiln_hir::TypeSignature {
                span: HirSpan::new(span.start, span.end),
                type_parameters: self.parameters(type_parameters),
                this_parameter: None,
                parameters: signature_params,
                rest: params.rest.is_some(),
                return_type: return_type.map(Box::new),
            }
        }
        fn object_members(
            &self,
            signatures: &[oxc_ast::ast::TSSignature<'_>],
        ) -> Vec<kiln_hir::TypeMember> {
            use oxc_ast::ast::TSSignature as S;
            signatures
                .iter()
                .map(|sig| match sig {
                    S::TSPropertySignature(p) => {
                        let span = p.span;
                        let name = p.key.static_name().map(|name| name.to_string());
                        let ty = p
                            .type_annotation
                            .as_deref()
                            .map(|a| self.convert(&a.type_annotation))
                            .unwrap_or_else(|| HirTypeExpr {
                                span: HirSpan::new(span.start, span.end),
                                kind: HirTypeExprKind::Unsupported {
                                    syntax: self.raw(span),
                                },
                            });
                        kiln_hir::TypeMember {
                            name,
                            key: self.owned_key(&p.key, p.computed),
                            span: HirSpan::new(span.start, span.end),
                            optional: p.optional,
                            readonly: p.readonly,
                            this_parameter: None,
                            kind: kiln_hir::TypeMemberKind::Property(ty),
                        }
                    }
                    S::TSMethodSignature(m) => {
                        let span = m.span;
                        let name = m.key.static_name().map(|name| name.to_string());
                        let return_type = m
                            .return_type
                            .as_deref()
                            .map(|a| self.convert(&a.type_annotation));
                        let mut signature = self.signature_with_return(
                            span,
                            m.type_parameters.as_deref(),
                            &m.params,
                            return_type.clone(),
                        );
                        signature.this_parameter =
                            self.this_parameter(m.this_param.as_deref()).map(Box::new);
                        let this_parameter = signature.this_parameter.clone();
                        let kind = match m.kind {
                            oxc_ast::ast::TSMethodSignatureKind::Method => {
                                kiln_hir::TypeMemberKind::Method(signature)
                            }
                            oxc_ast::ast::TSMethodSignatureKind::Get => {
                                kiln_hir::TypeMemberKind::Getter(return_type)
                            }
                            oxc_ast::ast::TSMethodSignatureKind::Set => {
                                kiln_hir::TypeMemberKind::Setter(signature)
                            }
                        };
                        kiln_hir::TypeMember {
                            name,
                            key: self.owned_key(&m.key, m.computed),
                            span: HirSpan::new(span.start, span.end),
                            optional: m.optional,
                            readonly: false,
                            this_parameter,
                            kind,
                        }
                    }
                    S::TSCallSignatureDeclaration(c) => {
                        let span = c.span;
                        let ret = c
                            .return_type
                            .as_deref()
                            .map(|a| self.convert(&a.type_annotation));
                        let mut signature = self.signature_with_return(
                            span,
                            c.type_parameters.as_deref(),
                            &c.params,
                            ret,
                        );
                        signature.this_parameter =
                            self.this_parameter(c.this_param.as_deref()).map(Box::new);
                        kiln_hir::TypeMember {
                            name: None,
                            key: None,
                            span: HirSpan::new(span.start, span.end),
                            optional: false,
                            readonly: false,
                            this_parameter: signature.this_parameter.clone(),
                            kind: kiln_hir::TypeMemberKind::Call(signature),
                        }
                    }
                    S::TSConstructSignatureDeclaration(c) => {
                        let span = c.span;
                        let ret = c
                            .return_type
                            .as_deref()
                            .map(|a| self.convert(&a.type_annotation));
                        let signature = self.signature_with_return(
                            span,
                            c.type_parameters.as_deref(),
                            &c.params,
                            ret,
                        );
                        kiln_hir::TypeMember {
                            name: None,
                            key: None,
                            span: HirSpan::new(span.start, span.end),
                            optional: false,
                            readonly: false,
                            this_parameter: None,
                            kind: kiln_hir::TypeMemberKind::Construct(signature),
                        }
                    }
                    S::TSIndexSignature(i) => {
                        let span = i.span;
                        kiln_hir::TypeMember {
                            name: Some(i.parameter.name.as_str().to_string()),
                            key: None,
                            span: HirSpan::new(span.start, span.end),
                            optional: false,
                            readonly: i.readonly,
                            this_parameter: None,
                            kind: kiln_hir::TypeMemberKind::Index {
                                parameter: i.parameter.name.as_str().to_string(),
                                key_type: self
                                    .convert(&i.parameter.type_annotation.type_annotation),
                                value_type: self.convert(&i.type_annotation.type_annotation),
                            },
                        }
                    }
                })
                .collect()
        }
    }
    impl<'a> Visit<'a> for Collector<'_> {
        fn visit_binding_identifier(&mut self, node: &oxc_ast::ast::BindingIdentifier<'a>) {
            self.result
                .binding_identifiers
                .push(kiln_hir::BindingIdentifierSite {
                    name: node.name.to_string(),
                    span: HirSpan::new(node.span.start, node.span.end),
                });
            walk::walk_binding_identifier(self, node);
        }
        fn visit_class(&mut self, node: &oxc_ast::ast::Class<'a>) {
            use oxc_ast::ast::{ClassElement, MethodDefinitionKind};
            let type_ref =
                |expression: &oxc_ast::ast::Expression<'_>,
                 args: Option<&oxc_ast::ast::TSTypeParameterInstantiation<'_>>| {
                    let span = expression.span();
                    HirTypeExpr {
                        span: HirSpan::new(span.start, span.end),
                        kind: HirTypeExprKind::Reference {
                            name: self.heritage_name_parts(expression).unwrap_or_else(|| {
                                self.raw(span)
                                    .split('.')
                                    .map(str::trim)
                                    .map(str::to_owned)
                                    .filter(|s| !s.is_empty())
                                    .collect()
                            }),
                            arguments: args
                                .map(|a| a.params.iter().map(|t| self.convert(t)).collect())
                                .unwrap_or_default(),
                        },
                    }
                };
            let extends = node
                .heritage
                .as_ref()
                .map(|heritage| type_ref(&heritage.expression, heritage.type_arguments.as_deref()));
            let implements = node
                .implements
                .iter()
                .map(|heritage| {
                    let span = heritage.span;
                    HirTypeExpr {
                        span: HirSpan::new(span.start, span.end),
                        kind: HirTypeExprKind::Reference {
                            name: self.type_name_parts(&heritage.expression),
                            arguments: heritage
                                .type_arguments
                                .as_ref()
                                .map(|arguments| {
                                    arguments.params.iter().map(|ty| self.convert(ty)).collect()
                                })
                                .unwrap_or_default(),
                        },
                    }
                })
                .collect();
            let mut members: Vec<kiln_hir::TypeClassMemberSite> = node
                .body
                .body
                .iter()
                .map(|element| match element {
                    ClassElement::PropertyDefinition(property) => kiln_hir::TypeClassMemberSite {
                        span: HirSpan::new(property.span.start, property.span.end),
                        name: property.key.static_name().map(|name| name.to_string()),
                        key: self.owned_key(&property.key, property.computed),
                        kind: kiln_hir::TypeClassMemberKind::Field,
                        static_: property.r#static,
                        optional: property.optional,
                        readonly: property.readonly,
                        abstract_: property.r#type
                            == oxc_ast::ast::PropertyDefinitionType::TSAbstractPropertyDefinition,
                        accessibility: Self::accessibility(property.accessibility),
                        override_: property.r#override,
                        key_type: None,
                        value_type: None,
                    },
                    ClassElement::AccessorProperty(property) => kiln_hir::TypeClassMemberSite {
                        span: HirSpan::new(property.span.start, property.span.end),
                        name: property.key.static_name().map(|name| name.to_string()),
                        key: self.owned_key(&property.key, property.computed),
                        kind: kiln_hir::TypeClassMemberKind::Accessor,
                        static_: property.r#static,
                        optional: false,
                        readonly: false,
                        abstract_: false,
                        accessibility: Self::accessibility(property.accessibility),
                        override_: property.r#override,
                        key_type: None,
                        value_type: None,
                    },
                    ClassElement::MethodDefinition(method) => kiln_hir::TypeClassMemberSite {
                        span: HirSpan::new(method.span.start, method.span.end),
                        name: method.key.static_name().map(|name| name.to_string()),
                        key: self.owned_key(&method.key, method.computed),
                        kind: match method.kind {
                            MethodDefinitionKind::Constructor => {
                                kiln_hir::TypeClassMemberKind::Constructor
                            }
                            MethodDefinitionKind::Get => kiln_hir::TypeClassMemberKind::Getter,
                            MethodDefinitionKind::Set => kiln_hir::TypeClassMemberKind::Setter,
                            MethodDefinitionKind::Method => kiln_hir::TypeClassMemberKind::Method,
                        },
                        static_: method.r#static,
                        optional: method.optional,
                        readonly: false,
                        abstract_: method.r#type
                            == oxc_ast::ast::MethodDefinitionType::TSAbstractMethodDefinition,
                        accessibility: Self::accessibility(method.accessibility),
                        override_: method.r#override,
                        key_type: None,
                        value_type: None,
                    },
                    ClassElement::StaticBlock(block) => kiln_hir::TypeClassMemberSite {
                        span: HirSpan::new(block.span.start, block.span.end),
                        name: None,
                        key: None,
                        kind: kiln_hir::TypeClassMemberKind::StaticBlock,
                        static_: true,
                        optional: false,
                        readonly: false,
                        abstract_: false,
                        accessibility: None,
                        override_: false,
                        key_type: None,
                        value_type: None,
                    },
                    ClassElement::TSIndexSignature(index) => kiln_hir::TypeClassMemberSite {
                        span: HirSpan::new(index.span.start, index.span.end),
                        name: Some(index.parameter.name.as_str().to_string()),
                        key: None,
                        kind: kiln_hir::TypeClassMemberKind::Index,
                        static_: index.r#static,
                        optional: false,
                        readonly: index.readonly,
                        abstract_: false,
                        accessibility: None,
                        override_: false,
                        key_type: Some(
                            self.convert(&index.parameter.type_annotation.type_annotation),
                        ),
                        value_type: Some(self.convert(&index.type_annotation.type_annotation)),
                    },
                })
                .collect();
            for element in &node.body.body {
                let ClassElement::MethodDefinition(method) = element else {
                    continue;
                };
                if method.kind != MethodDefinitionKind::Constructor {
                    continue;
                }
                for parameter in &method.value.params.items {
                    if parameter.accessibility.is_none() && !parameter.readonly {
                        continue;
                    }
                    members.push(kiln_hir::TypeClassMemberSite {
                        span: HirSpan::new(parameter.span.start, parameter.span.end),
                        name: match &parameter.pattern {
                            oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                                Some(id.name.to_string())
                            }
                            _ => None,
                        },
                        key: match &parameter.pattern {
                            oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                                Some(kiln_hir::ObjectKey::Static(id.name.to_string().into()))
                            }
                            _ => None,
                        },
                        kind: kiln_hir::TypeClassMemberKind::ParameterProperty,
                        static_: false,
                        optional: parameter.optional,
                        readonly: parameter.readonly,
                        abstract_: false,
                        accessibility: Self::accessibility(parameter.accessibility),
                        override_: parameter.r#override,
                        key_type: None,
                        value_type: None,
                    });
                }
            }
            self.result.classes.push(kiln_hir::TypeClassSite {
                span: HirSpan::new(node.span.start, node.span.end),
                name: node.id.as_ref().map(|id| id.name.to_string()),
                name_pos: node.id.as_ref().map_or(node.span.start, |id| id.span.start),
                exported: self.is_exported(node.span),
                default_exported: self.is_default_exported(node.span),
                type_parameters: self.parameters(node.type_parameters.as_deref()),
                extends,
                implements,
                abstract_: node.r#abstract,
                declared: node.declare,
                members,
            });
            if node.declare {
                let span = node.span;
                self.result.declarations.push(HirTypeDeclaration {
                    name: node.id.as_ref().map(|id| id.name.to_string()),
                    span: HirSpan::new(span.start, span.end),
                    kind: kiln_hir::TypeDeclarationKind::Class,
                    type_parameters: Vec::new(),
                    extends: Vec::new(),
                    members: Vec::new(),
                    source_text: self.raw(span),
                    exported: self.is_exported(span),
                    default_exported: self.is_default_exported(span),
                    declared: true,
                    const_: false,
                    global: false,
                    body: Vec::new(),
                    enum_members: Vec::new(),
                });
            }
            walk::walk_class(self, node);
        }
        fn visit_export_declaration(&mut self, node: &oxc_ast::ast::ExportDeclaration<'a>) {
            let declaration_span = node.declaration.span();
            self.result
                .exported_declarations
                .push(HirSpan::new(declaration_span.start, declaration_span.end));
            walk::walk_export_declaration(self, node);
        }
        fn visit_export_default_declaration(
            &mut self,
            node: &oxc_ast::ast::ExportDefaultDeclaration<'a>,
        ) {
            let declaration_span = node.declaration.span();
            self.result
                .default_exported_declarations
                .push(HirSpan::new(declaration_span.start, declaration_span.end));
            walk::walk_export_default_declaration(self, node);
        }
        fn visit_variable_declaration(&mut self, node: &oxc_ast::ast::VariableDeclaration<'a>) {
            if node.declare {
                let span = node.span;
                let mut lowerer = Lowerer {
                    text: self.source,
                    path: self.path.as_deref(),
                    native_import_meta: self.native_import_meta,
                };
                self.result.declarations.push(HirTypeDeclaration {
                    name: None,
                    span: HirSpan::new(span.start, span.end),
                    kind: kiln_hir::TypeDeclarationKind::Variable,
                    type_parameters: Vec::new(),
                    extends: Vec::new(),
                    members: Vec::new(),
                    source_text: self.raw(span),
                    exported: self.is_exported(span),
                    default_exported: self.is_default_exported(span),
                    declared: true,
                    const_: node.kind == oxc_ast::ast::VariableDeclarationKind::Const,
                    global: false,
                    enum_members: Vec::new(),
                    body: vec![HirStatement {
                        kind: lowerer.variable(node),
                        span: HirSpan::new(span.start, span.end),
                    }],
                });
            }
            walk::walk_variable_declaration(self, node);
        }
        fn visit_ts_type_annotation(&mut self, node: &oxc_ast::ast::TSTypeAnnotation<'a>) {
            let ty_span = node.type_annotation.span();
            if !self.classified.contains(&(node.span.start, node.span.end)) {
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::Other("annotation".into()),
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&node.type_annotation),
                });
            }
            walk::walk_ts_type_annotation(self, node);
        }
        fn visit_formal_parameters(&mut self, node: &oxc_ast::ast::FormalParameters<'a>) {
            for (index, parameter) in node.items.iter().enumerate() {
                if let Some(annotation) = parameter.type_annotation.as_deref() {
                    let ty_span = annotation.type_annotation.span();
                    self.classified
                        .insert((annotation.span.start, annotation.span.end));
                    self.result.annotations.push(HirTypeAnnotationSite {
                        site: HirTypeSite::Parameter {
                            index: index as u32,
                            name: match &parameter.pattern {
                                oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                                    Some(id.name.to_string())
                                }
                                _ => None,
                            },
                        },
                        owner_span: HirSpan::new(parameter.span.start, parameter.span.end),
                        span: HirSpan::new(ty_span.start, ty_span.end),
                        operand_span: None,
                        ty: self.convert(&annotation.type_annotation),
                    });
                }
            }
            walk::walk_formal_parameters(self, node);
        }
        fn visit_ts_property_signature(&mut self, node: &oxc_ast::ast::TSPropertySignature<'a>) {
            if let Some(annotation) = node.type_annotation.as_deref() {
                let ty_span = annotation.type_annotation.span();
                self.classified
                    .insert((annotation.span.start, annotation.span.end));
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::Property {
                        name: node.key.static_name().map(|n| n.to_string()),
                    },
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&annotation.type_annotation),
                });
            }
            walk::walk_ts_property_signature(self, node);
        }
        fn visit_property_definition(&mut self, node: &oxc_ast::ast::PropertyDefinition<'a>) {
            if let Some(annotation) = node.type_annotation.as_deref() {
                let ty_span = annotation.type_annotation.span();
                self.classified
                    .insert((annotation.span.start, annotation.span.end));
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::ClassField {
                        name: node.key.static_name().map(|n| n.to_string()),
                    },
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&annotation.type_annotation),
                });
            }
            walk::walk_property_definition(self, node);
        }
        fn visit_accessor_property(&mut self, node: &oxc_ast::ast::AccessorProperty<'a>) {
            if let Some(annotation) = node.type_annotation.as_deref() {
                let ty_span = annotation.type_annotation.span();
                self.classified
                    .insert((annotation.span.start, annotation.span.end));
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::ClassField {
                        name: node.key.static_name().map(|name| name.to_string()),
                    },
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&annotation.type_annotation),
                });
            }
            walk::walk_accessor_property(self, node);
        }
        fn visit_function(
            &mut self,
            node: &oxc_ast::ast::Function<'a>,
            flags: oxc_syntax::scope::ScopeFlags,
        ) {
            let signature_return = node
                .return_type
                .as_deref()
                .map(|a| self.convert(&a.type_annotation));
            let mut signature = self.signature_with_return(
                node.span,
                node.type_parameters.as_deref(),
                &node.params,
                signature_return,
            );
            signature.this_parameter = self
                .this_parameter(node.this_param.as_deref())
                .map(Box::new);
            self.result
                .signatures
                .push(kiln_hir::FunctionSignatureSite {
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    signature,
                    has_body: node.body.is_some(),
                });
            if node.body.is_none() {
                let span = node.span;
                self.result.declarations.push(HirTypeDeclaration {
                    name: node.id.as_ref().map(|id| id.name.to_string()),
                    span: HirSpan::new(span.start, span.end),
                    kind: HirTypeDeclarationKind::Overload,
                    type_parameters: self.parameters(node.type_parameters.as_deref()),
                    extends: Vec::new(),
                    members: Vec::new(),
                    source_text: self.raw(span),
                    exported: self.is_exported(span),
                    default_exported: self.is_default_exported(span),
                    declared: node.declare,
                    const_: false,
                    global: false,
                    enum_members: Vec::new(),
                    body: Vec::new(),
                });
            }
            if let Some(annotation) = node.return_type.as_deref() {
                let ty_span = annotation.type_annotation.span();
                self.classified
                    .insert((annotation.span.start, annotation.span.end));
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::Return,
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&annotation.type_annotation),
                });
            }
            walk::walk_function(self, node, flags);
        }
        fn visit_import_declaration(&mut self, node: &oxc_ast::ast::ImportDeclaration<'a>) {
            if node.import_kind.is_type() || node.specifiers.as_ref().is_some_and(|specifiers| {
                specifiers.iter().any(|specifier| matches!(specifier, oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(s) if s.import_kind.is_type()))
            }) {
                let span = node.span;
                self.result.declarations.push(HirTypeDeclaration {
                    name: None,
                    span: HirSpan::new(span.start, span.end),
                    kind: HirTypeDeclarationKind::Import,
                    type_parameters: Vec::new(),
                    extends: Vec::new(),
                    members: Vec::new(),
                    source_text: self.raw(span),
                    exported: false, default_exported: false, declared: false,
                    const_: false, global: false, body: Vec::new(),
                    enum_members: Vec::new(),
                });
            }
            walk::walk_import_declaration(self, node);
        }
        fn visit_ts_import_equals_declaration(
            &mut self,
            node: &oxc_ast::ast::TSImportEqualsDeclaration<'a>,
        ) {
            let reference = match &node.module_reference {
                oxc_ast::ast::TSModuleReference::ExternalModuleReference(reference) => {
                    kiln_hir::TypeImportEqualsReference::ExternalModule(
                        reference.expression.value.to_string(),
                    )
                }
                oxc_ast::ast::TSModuleReference::IdentifierReference(reference) => {
                    kiln_hir::TypeImportEqualsReference::EntityName(vec![
                        reference.name.to_string(),
                    ])
                }
                oxc_ast::ast::TSModuleReference::QualifiedName(reference) => {
                    kiln_hir::TypeImportEqualsReference::EntityName(
                        self.qualified_type_name_parts(reference),
                    )
                }
            };
            self.result.import_equals.push(kiln_hir::TypeImportEquals {
                name: node.id.name.to_string(),
                span: HirSpan::new(node.span.start, node.span.end),
                reference,
                type_only: node.import_kind.is_type(),
                exported: self.is_exported(node.span),
                declared: self
                    .source
                    .get(node.span.start as usize..)
                    .is_some_and(|s| s.trim_start().starts_with("declare ")),
            });
            walk::walk_ts_import_equals_declaration(self, node);
        }
        fn visit_arrow_function_expression(
            &mut self,
            node: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            let signature_return = node
                .return_type
                .as_deref()
                .map(|a| self.convert(&a.type_annotation));
            self.result
                .signatures
                .push(kiln_hir::FunctionSignatureSite {
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    signature: self.signature_with_return(
                        node.span,
                        node.type_parameters.as_deref(),
                        &node.params,
                        signature_return,
                    ),
                    has_body: true,
                });
            if let Some(annotation) = node.return_type.as_deref() {
                let ty_span = annotation.type_annotation.span();
                self.classified
                    .insert((annotation.span.start, annotation.span.end));
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::Return,
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&annotation.type_annotation),
                });
            }
            walk::walk_arrow_function_expression(self, node);
        }
        fn visit_call_expression(&mut self, node: &oxc_ast::ast::CallExpression<'a>) {
            if let Some(arguments) = node.type_arguments.as_deref() {
                self.result
                    .instantiations
                    .push(kiln_hir::TypeInstantiationSite {
                        owner_span: HirSpan::new(node.span.start, node.span.end),
                        operand_span: None,
                        arguments: arguments.params.iter().map(|t| self.convert(t)).collect(),
                        kind: kiln_hir::TypeInstantiationKind::Call,
                    });
            }
            walk::walk_call_expression(self, node);
        }
        fn visit_new_expression(&mut self, node: &oxc_ast::ast::NewExpression<'a>) {
            if let Some(arguments) = node.type_arguments.as_deref() {
                self.result
                    .instantiations
                    .push(kiln_hir::TypeInstantiationSite {
                        owner_span: HirSpan::new(node.span.start, node.span.end),
                        operand_span: None,
                        arguments: arguments.params.iter().map(|t| self.convert(t)).collect(),
                        kind: kiln_hir::TypeInstantiationKind::Construct,
                    });
            }
            walk::walk_new_expression(self, node);
        }
        fn visit_ts_instantiation_expression(
            &mut self,
            node: &oxc_ast::ast::TSInstantiationExpression<'a>,
        ) {
            self.result
                .instantiations
                .push(kiln_hir::TypeInstantiationSite {
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    operand_span: Some(self.operand_span(&node.expression)),
                    arguments: node
                        .type_arguments
                        .params
                        .iter()
                        .map(|t| self.convert(t))
                        .collect(),
                    kind: kiln_hir::TypeInstantiationKind::Expression,
                });
            walk::walk_ts_instantiation_expression(self, node);
        }
        fn visit_variable_declarator(&mut self, node: &oxc_ast::ast::VariableDeclarator<'a>) {
            if let Some(annotation) = node.type_annotation.as_deref() {
                let ty_span = annotation.type_annotation.span();
                self.classified
                    .insert((annotation.span.start, annotation.span.end));
                self.result.annotations.push(HirTypeAnnotationSite {
                    site: HirTypeSite::Variable {
                        name: match &node.id {
                            oxc_ast::ast::BindingPattern::BindingIdentifier(id) => {
                                Some(id.name.to_string())
                            }
                            _ => None,
                        },
                    },
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    span: HirSpan::new(ty_span.start, ty_span.end),
                    operand_span: None,
                    ty: self.convert(&annotation.type_annotation),
                });
            }
            walk::walk_variable_declarator(self, node);
        }
        fn visit_ts_as_expression(&mut self, node: &oxc_ast::ast::TSAsExpression<'a>) {
            if self.raw(node.type_annotation.span()).trim() == "const" {
                self.result
                    .expression_wrappers
                    .push(kiln_hir::TypeExpressionWrapperSite {
                        owner_span: HirSpan::new(node.span.start, node.span.end),
                        operand_span: self.operand_span(&node.expression),
                        kind: kiln_hir::TypeExpressionWrapperKind::AsConst,
                    });
                walk::walk_ts_as_expression(self, node);
                return;
            }
            let span = node.type_annotation.span();
            self.result.annotations.push(HirTypeAnnotationSite {
                site: HirTypeSite::AsExpression,
                owner_span: HirSpan::new(node.span.start, node.span.end),
                span: HirSpan::new(span.start, span.end),
                operand_span: Some(self.operand_span(&node.expression)),
                ty: self.convert(&node.type_annotation),
            });
            walk::walk_ts_as_expression(self, node);
        }
        fn visit_ts_satisfies_expression(
            &mut self,
            node: &oxc_ast::ast::TSSatisfiesExpression<'a>,
        ) {
            let span = node.type_annotation.span();
            self.result.annotations.push(HirTypeAnnotationSite {
                site: HirTypeSite::SatisfiesExpression,
                owner_span: HirSpan::new(node.span.start, node.span.end),
                span: HirSpan::new(span.start, span.end),
                operand_span: Some(self.operand_span(&node.expression)),
                ty: self.convert(&node.type_annotation),
            });
            walk::walk_ts_satisfies_expression(self, node);
        }
        fn visit_ts_type_assertion(&mut self, node: &oxc_ast::ast::TSTypeAssertion<'a>) {
            let span = node.type_annotation.span();
            self.result.annotations.push(HirTypeAnnotationSite {
                site: HirTypeSite::AsExpression,
                owner_span: HirSpan::new(node.span.start, node.span.end),
                span: HirSpan::new(span.start, span.end),
                operand_span: Some(self.operand_span(&node.expression)),
                ty: self.convert(&node.type_annotation),
            });
            walk::walk_ts_type_assertion(self, node);
        }
        fn visit_ts_non_null_expression(&mut self, node: &oxc_ast::ast::TSNonNullExpression<'a>) {
            self.result
                .expression_wrappers
                .push(kiln_hir::TypeExpressionWrapperSite {
                    owner_span: HirSpan::new(node.span.start, node.span.end),
                    operand_span: self.operand_span(&node.expression),
                    kind: kiln_hir::TypeExpressionWrapperKind::NonNull,
                });
            walk::walk_ts_non_null_expression(self, node);
        }
        fn visit_ts_type_alias_declaration(
            &mut self,
            node: &oxc_ast::ast::TSTypeAliasDeclaration<'a>,
        ) {
            let span = node.span;
            let expr = self.convert(&node.type_annotation);
            self.result.declarations.push(HirTypeDeclaration {
                name: Some(node.id.name.to_string()),
                span: HirSpan::new(span.start, span.end),
                kind: HirTypeDeclarationKind::Alias(expr),
                type_parameters: self.parameters(node.type_parameters.as_deref()),
                extends: Vec::new(),
                members: Vec::new(),
                source_text: self.raw(span),
                exported: self.is_exported(span),
                default_exported: self.is_default_exported(span),
                declared: node.declare,
                const_: false,
                global: false,
                enum_members: Vec::new(),
                body: Vec::new(),
            });
            walk::walk_ts_type_alias_declaration(self, node);
        }
        fn visit_ts_interface_declaration(
            &mut self,
            node: &oxc_ast::ast::TSInterfaceDeclaration<'a>,
        ) {
            let span = node.span;
            self.result.declarations.push(HirTypeDeclaration {
                name: Some(node.id.name.to_string()),
                span: HirSpan::new(span.start, span.end),
                kind: HirTypeDeclarationKind::Interface,
                type_parameters: self.parameters(node.type_parameters.as_deref()),
                extends: node
                    .extends
                    .iter()
                    .map(|e| {
                        let span = e.span;
                        HirTypeExpr {
                            span: HirSpan::new(span.start, span.end),
                            kind: HirTypeExprKind::Reference {
                                name: self.type_name_parts(&e.type_name),
                                arguments: e
                                    .type_arguments
                                    .as_ref()
                                    .map(|a| a.params.iter().map(|t| self.convert(t)).collect())
                                    .unwrap_or_default(),
                            },
                        }
                    })
                    .collect(),
                members: self.object_members(&node.body.body),
                source_text: self.raw(span),
                exported: self.is_exported(span),
                default_exported: self.is_default_exported(span),
                declared: node.declare,
                const_: false,
                global: false,
                enum_members: Vec::new(),
                body: Vec::new(),
            });
            walk::walk_ts_interface_declaration(self, node);
        }
        fn visit_ts_enum_declaration(&mut self, node: &oxc_ast::ast::TSEnumDeclaration<'a>) {
            let span = node.span;
            self.result.declarations.push(HirTypeDeclaration {
                name: Some(node.id.name.to_string()),
                span: HirSpan::new(span.start, span.end),
                kind: HirTypeDeclarationKind::Enum,
                type_parameters: Vec::new(),
                extends: Vec::new(),
                members: Vec::new(),
                source_text: self.raw(span),
                exported: self.is_exported(span),
                default_exported: self.is_default_exported(span),
                declared: node.declare,
                const_: node.r#const,
                global: false,
                enum_members: node
                    .body
                    .members
                    .iter()
                    .map(|member| {
                        let (name, name_kind) = match &member.id {
                            oxc_ast::ast::TSEnumMemberName::Identifier(id) => (
                                StringUnits::from(id.name.as_str()),
                                kiln_hir::EnumMemberNameKind::Identifier,
                            ),
                            oxc_ast::ast::TSEnumMemberName::String(id) => (
                                oxc_string_units(id.value.as_str(), id.lone_surrogates),
                                kiln_hir::EnumMemberNameKind::StringLiteral,
                            ),
                            oxc_ast::ast::TSEnumMemberName::ComputedString(id) => (
                                oxc_string_units(id.value.as_str(), id.lone_surrogates),
                                kiln_hir::EnumMemberNameKind::ComputedString,
                            ),
                            oxc_ast::ast::TSEnumMemberName::ComputedTemplateString(id) => (
                                id.quasis.first().map_or_else(StringUnits::default, |q| {
                                    oxc_string_units(
                                        q.value
                                            .cooked
                                            .as_ref()
                                            .map_or(q.value.raw.as_str(), |s| s.as_str()),
                                        q.lone_surrogates,
                                    )
                                }),
                                kiln_hir::EnumMemberNameKind::ComputedString,
                            ),
                        };
                        kiln_hir::EnumMember {
                            name,
                            name_kind,
                            span: HirSpan::new(member.span.start, member.span.end),
                            initializer: member.initializer.as_ref().map(|e| {
                                let mut lowerer = Lowerer {
                                    text: self.source,
                                    path: self.path.as_deref(),
                                    native_import_meta: self.native_import_meta,
                                };
                                lowerer.expression(e)
                            }),
                        }
                    })
                    .collect(),
                body: Vec::new(),
            });
            walk::walk_ts_enum_declaration(self, node);
        }
        fn visit_ts_namespace_declaration(
            &mut self,
            node: &oxc_ast::ast::TSNamespaceDeclaration<'a>,
        ) {
            let span = node.span;
            self.result.declarations.push(HirTypeDeclaration {
                name: Some(node.id.name.to_string()),
                span: HirSpan::new(span.start, span.end),
                kind: HirTypeDeclarationKind::Namespace,
                type_parameters: Vec::new(),
                extends: Vec::new(),
                members: Vec::new(),
                source_text: self.raw(span),
                exported: self.is_exported(span),
                default_exported: self.is_default_exported(span),
                declared: node.declare,
                const_: false,
                global: false,
                enum_members: Vec::new(),
                body: match &node.body {
                    oxc_ast::ast::TSNamespaceDeclarationBody::TSModuleBlock(block) => {
                        let mut lowerer = Lowerer {
                            text: self.source,
                            path: self.path.as_deref(),
                            native_import_meta: self.native_import_meta,
                        };
                        lowerer.statements(&block.body)
                    }
                    oxc_ast::ast::TSNamespaceDeclarationBody::TSNamespaceDeclaration(_) => {
                        Vec::new()
                    }
                },
            });
            walk::walk_ts_namespace_declaration(self, node);
        }
        fn visit_ts_external_module_declaration(
            &mut self,
            node: &oxc_ast::ast::TSExternalModuleDeclaration<'a>,
        ) {
            let span = node.span;
            let body = node
                .body
                .as_deref()
                .map(|block| {
                    let mut lowerer = Lowerer {
                        text: self.source,
                        path: self.path.as_deref(),
                        native_import_meta: self.native_import_meta,
                    };
                    lowerer.statements(&block.body)
                })
                .unwrap_or_default();
            self.result.declarations.push(HirTypeDeclaration {
                name: Some(node.id.value.to_string()),
                span: HirSpan::new(span.start, span.end),
                kind: HirTypeDeclarationKind::Module,
                type_parameters: Vec::new(),
                extends: Vec::new(),
                members: Vec::new(),
                source_text: self.raw(span),
                exported: self.is_exported(span),
                default_exported: self.is_default_exported(span),
                declared: node.declare,
                const_: false,
                global: false,
                enum_members: Vec::new(),
                body,
            });
            walk::walk_ts_external_module_declaration(self, node);
        }
        fn visit_ts_global_declaration(&mut self, node: &oxc_ast::ast::TSGlobalDeclaration<'a>) {
            let span = node.span;
            let mut lowerer = Lowerer {
                text: self.source,
                path: self.path.as_deref(),
                native_import_meta: self.native_import_meta,
            };
            let body = lowerer.statements(&node.body.body);
            self.result.declarations.push(HirTypeDeclaration {
                name: None,
                span: HirSpan::new(span.start, span.end),
                kind: HirTypeDeclarationKind::Module,
                type_parameters: Vec::new(),
                extends: Vec::new(),
                members: Vec::new(),
                source_text: self.raw(span),
                exported: self.is_exported(span),
                default_exported: self.is_default_exported(span),
                declared: node.declare,
                const_: false,
                global: true,
                enum_members: Vec::new(),
                body,
            });
            walk::walk_ts_global_declaration(self, node);
        }
    }
    let mut collector = Collector {
        source,
        path: path.map(str::to_owned),
        native_import_meta,
        result: HirTypeSyntax {
            module_attributes,
            ..HirTypeSyntax::default()
        },
        classified: HashSet::new(),
    };
    for comment in &program.comments {
        if comment.kind != oxc_ast::ast::CommentKind::Line {
            continue;
        }
        let raw = collector.raw(comment.span);
        let Some(content) = raw.strip_prefix("///") else {
            continue;
        };
        let content = content.trim();
        if !content.starts_with("<reference") {
            continue;
        }
        let mut attrs = HashMap::<&str, String>::new();
        for token in content.split_whitespace().skip(1) {
            let token = token.trim_end_matches(['/', '>']);
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            attrs.insert(key, value.trim_matches(['"', '\'']).to_owned());
        }
        if attrs
            .get("no-default-lib")
            .is_some_and(|value| value == "true")
        {
            collector.result.no_default_lib = true;
        }
        for (key, kind) in [
            ("path", kiln_hir::TypeReferenceKind::Path),
            ("types", kiln_hir::TypeReferenceKind::Types),
            ("lib", kiln_hir::TypeReferenceKind::Lib),
        ] {
            if let Some(value) = attrs.get(key) {
                let resolution_mode = if kind == kiln_hir::TypeReferenceKind::Types {
                    match attrs.get("resolution-mode").map(String::as_str) {
                        Some("import") => Some(kiln_hir::TypeResolutionMode::Import),
                        Some("require") => Some(kiln_hir::TypeResolutionMode::Require),
                        _ => None,
                    }
                } else {
                    None
                };
                collector
                    .result
                    .references
                    .push(kiln_hir::TypeReferenceDirective {
                        kind,
                        value: value.clone(),
                        position: comment.span.start,
                        resolution_mode,
                    });
            }
        }
    }
    collector.visit_program(program);
    collector
        .result
        .annotations
        .sort_by_key(|site| (site.span.start, site.span.end));
    collector
        .result
        .binding_identifiers
        .sort_by_key(|site| (site.span.start, site.span.end));
    collector
        .result
        .exported_declarations
        .sort_by_key(|span| (span.start, span.end));
    collector
        .result
        .default_exported_declarations
        .sort_by_key(|span| (span.start, span.end));
    collector
        .result
        .module_attributes
        .sort_by_key(|attributes| attributes.statement_span.start);
    collector
        .result
        .declarations
        .sort_by_key(|decl| (decl.span.start, decl.span.end));
    collector
        .result
        .classes
        .sort_by_key(|class| (class.span.start, class.span.end));
    collector
        .result
        .instantiations
        .sort_by_key(|site| (site.owner_span.start, site.owner_span.end));
    collector
        .result
        .expression_wrappers
        .sort_by_key(|site| (site.owner_span.start, site.owner_span.end));
    collector.result
}

fn capture_original_body(
    path: Option<&str>,
    source: &str,
    parsed: &oxc_parser::ParserReturn<'_>,
    native_import_meta: Option<bool>,
) -> Vec<HirStatement> {
    let mut lowerer = Lowerer {
        text: source,
        path,
        native_import_meta,
    };
    lowerer.statements(&parsed.program.body)
}

/// `Lowerer` intentionally drops erased TS declarations from the executable HIR. The checker
/// still needs those declarations in their lexical statement list, so retain span-only markers in
/// the owned source snapshot. The checker replaces each marker from `TypeSyntax.declarations`.
fn retain_erased_declarations(syntax: &mut HirTypeSyntax) {
    fn recurse_expression(expression: &mut HirExpression, declarations: &[HirTypeDeclaration]) {
        use HirExpressionKind as E;
        match &mut expression.kind {
            E::Function(function) => retain_list(&mut function.body, declarations, function.span),
            E::Unary { argument, .. }
            | E::Await(argument)
            | E::Chain {
                expression: argument,
            } => recurse_expression(argument, declarations),
            E::Binary { left, right, .. } | E::Logical { left, right, .. } => {
                recurse_expression(left, declarations);
                recurse_expression(right, declarations);
            }
            E::Conditional {
                test,
                consequent,
                alternate,
            } => {
                recurse_expression(test, declarations);
                recurse_expression(consequent, declarations);
                recurse_expression(alternate, declarations);
            }
            E::Call {
                callee, arguments, ..
            }
            | E::New { callee, arguments } => {
                recurse_expression(callee, declarations);
                for argument in arguments {
                    match argument {
                        ListElement::Item(e) | ListElement::Spread(e) => {
                            recurse_expression(e, declarations)
                        }
                        ListElement::Hole => {}
                    }
                }
            }
            E::Array { elements }
            | E::SuperCall {
                arguments: elements,
            } => {
                for element in elements {
                    match element {
                        ListElement::Item(e) | ListElement::Spread(e) => {
                            recurse_expression(e, declarations)
                        }
                        ListElement::Hole => {}
                    }
                }
            }
            E::Sequence { operands } => {
                for e in operands {
                    recurse_expression(e, declarations);
                }
            }
            E::Object { members } => {
                for member in members {
                    match member {
                        kiln_hir::ObjectMember::Property(p) => {
                            recurse_expression(&mut p.value, declarations)
                        }
                        kiln_hir::ObjectMember::Spread(e) => recurse_expression(e, declarations),
                        kiln_hir::ObjectMember::Accessor { function, .. } => {
                            retain_list(&mut function.body, declarations, function.span)
                        }
                    }
                }
            }
            E::Template { expressions, .. } => {
                for e in expressions {
                    recurse_expression(e, declarations);
                }
            }
            E::ImportCall { specifier, options } => {
                recurse_expression(specifier, declarations);
                if let Some(e) = options {
                    recurse_expression(e, declarations);
                }
            }
            E::Class(class) => {
                if let Some(constructor) = &mut class.constructor {
                    retain_list(&mut constructor.body, declarations, constructor.span);
                }
                for method in &mut class.methods {
                    retain_list(
                        &mut method.function.body,
                        declarations,
                        method.function.span,
                    );
                }
                for initializer in &mut class.initialisers {
                    if let kiln_hir::ClassInitialiser::StaticBlock(function) = initializer {
                        retain_list(&mut function.body, declarations, function.span);
                    }
                }
            }
            _ => {}
        }
    }
    fn recurse_statement(statement: &mut HirStatement, declarations: &[HirTypeDeclaration]) {
        let statement_span = statement.span;
        match &mut statement.kind {
            HirStatementKind::Variable {
                declarations: vars, ..
            } => {
                for var in vars {
                    if let Some(initializer) = &mut var.initializer {
                        recurse_expression(initializer, declarations);
                    }
                }
            }
            HirStatementKind::Expression(expression) => {
                recurse_expression(expression, declarations)
            }
            HirStatementKind::Return(Some(expression)) | HirStatementKind::Throw(expression) => {
                recurse_expression(expression, declarations)
            }
            HirStatementKind::Block(body) => retain_list(body, declarations, statement_span),
            HirStatementKind::Function(function) => {
                retain_list(&mut function.body, declarations, function.span);
            }
            HirStatementKind::Class(class) => {
                if let Some(constructor) = &mut class.constructor {
                    retain_list(&mut constructor.body, declarations, constructor.span);
                }
                for method in &mut class.methods {
                    retain_list(
                        &mut method.function.body,
                        declarations,
                        method.function.span,
                    );
                }
                for initializer in &mut class.initialisers {
                    match initializer {
                        kiln_hir::ClassInitialiser::Field(field) => {
                            if let Some(value) = &mut field.value {
                                recurse_expression(value, declarations);
                            }
                        }
                        kiln_hir::ClassInitialiser::StaticBlock(function) => {
                            retain_list(&mut function.body, declarations, function.span);
                        }
                    }
                }
            }
            HirStatementKind::If {
                consequent,
                alternate,
                ..
            } => {
                recurse_statement(consequent, declarations);
                if let Some(alternate) = alternate {
                    recurse_statement(alternate, declarations);
                }
            }
            HirStatementKind::While { body, .. }
            | HirStatementKind::DoWhile { body, .. }
            | HirStatementKind::With { body, .. }
            | HirStatementKind::Labelled { body, .. }
            | HirStatementKind::For { body, .. }
            | HirStatementKind::ForIn { body, .. }
            | HirStatementKind::ForOf { body, .. } => recurse_statement(body, declarations),
            HirStatementKind::Switch { cases, .. } => {
                for case in cases {
                    retain_list(&mut case.body, declarations, statement_span);
                }
            }
            HirStatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                retain_list(block, declarations, statement_span);
                if let Some(handler) = handler {
                    retain_list(&mut handler.body, declarations, statement_span);
                }
                retain_list(finalizer, declarations, statement_span);
            }
            _ => {}
        }
    }
    fn retain_list(
        body: &mut Vec<HirStatement>,
        declarations: &[HirTypeDeclaration],
        scope: HirSpan,
    ) {
        for statement in body.iter_mut() {
            recurse_statement(statement, declarations);
        }
        for declaration in declarations {
            if matches!(declaration.kind, HirTypeDeclarationKind::Import) {
                continue;
            }
            // The body of a namespace/module is scoped by its own declaration span; its own
            // declaration must never be inserted back into that body.
            if declaration.span == scope {
                continue;
            }
            if declaration.span.start < scope.start || declaration.span.end > scope.end {
                continue;
            }
            // A nested function/block/namespace owns declarations inside its span; recurse into
            // that statement or namespace body instead of lifting the marker into this scope.
            if body.iter().any(|statement| {
                statement.span != declaration.span
                    && statement.span.start <= declaration.span.start
                    && statement.span.end >= declaration.span.end
            }) {
                continue;
            }
            if declarations.iter().any(|parent| {
                parent.span != declaration.span
                    && parent.span != scope
                    && matches!(
                        parent.kind,
                        HirTypeDeclarationKind::Namespace | HirTypeDeclarationKind::Module
                    )
                    && parent.span.start >= scope.start
                    && parent.span.end <= scope.end
                    && parent.span.start <= declaration.span.start
                    && parent.span.end >= declaration.span.end
            }) {
                continue;
            }
            if body
                .iter()
                .any(|statement| statement.span == declaration.span)
            {
                continue;
            }
            body.push(HirStatement {
                kind: HirStatementKind::Unsupported {
                    feature: "TypeScript declaration marker".into(),
                },
                span: declaration.span,
            });
        }
        body.sort_by_key(|statement| (statement.span.start, statement.span.end));
    }
    // The root list is not a declaration body, even when its sole declaration spans the whole
    // file. Use an unbounded owner span so a one-statement file is not mistaken for its own body.
    let scope = HirSpan::new(0, u32::MAX);
    let declarations = syntax.declarations.clone();
    retain_list(&mut syntax.body, &declarations, scope);
    for declaration in &mut syntax.declarations {
        if matches!(
            declaration.kind,
            HirTypeDeclarationKind::Namespace | HirTypeDeclarationKind::Module
        ) {
            retain_list(&mut declaration.body, &declarations, declaration.span);
        }
    }
}

fn lower_parsed(
    path: Option<&str>,
    parsed: &oxc_parser::ParserReturn<'_>,
    source: &str,
    source_type: SourceType,
) -> Module {
    lower_parsed_in_context(path, parsed, source, source_type, None, None, None)
}

fn lower_parsed_in_context(
    path: Option<&str>,
    parsed: &oxc_parser::ParserReturn<'_>,
    source: &str,
    source_type: SourceType,
    allow: Option<&EvalSyntax>,
    demanded_exports: Option<&BTreeSet<String>>,
    native_import_meta: Option<bool>,
) -> Module {
    let filter_module_record = demanded_exports.is_some();
    let module_statement_spans = parsed
        .program
        .body
        .iter()
        .filter(|statement| {
            matches!(
                statement,
                Statement::ImportDeclaration(_)
                    | Statement::ExportAllDeclaration(_)
                    | Statement::ExportDefaultDeclaration(_)
                    | Statement::ExportDeclaration(_)
                    | Statement::ExportNamedDeclaration(_)
                    | Statement::ExportFromDeclaration(_)
            )
        })
        .map(|statement| (statement.span().start, statement.span().end))
        .collect::<HashSet<_>>();
    let import_binding_spans = parsed
        .program
        .body
        .iter()
        .filter_map(|statement| match statement {
            Statement::ImportDeclaration(declaration) => declaration.specifiers.as_ref(),
            _ => None,
        })
        .flatten()
        .map(|specifier| match specifier {
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => specifier.local.span,
            ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => specifier.local.span,
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => specifier.local.span,
        })
        .map(|span| (span.start, span.end))
        .collect::<HashSet<_>>();
    let mut requested_modules = parsed
        .module_record
        .requested_modules
        .iter()
        .filter_map(|(specifier, occurrences)| {
            let occurrences = occurrences
                .iter()
                .filter(|occurrence| {
                    !filter_module_record
                        || module_statement_spans.contains(&(
                            occurrence.statement_span.start,
                            occurrence.statement_span.end,
                        ))
                })
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
                .collect::<Vec<_>>();
            (!occurrences.is_empty()).then(|| RequestedModule {
                specifier: specifier.to_string(),
                occurrences,
            })
        })
        .collect::<Vec<_>>();
    let mut imports = parsed
        .module_record
        .import_entries
        .iter()
        .filter(|entry| {
            !filter_module_record
                || import_binding_spans
                    .contains(&(entry.local_name.span.start, entry.local_name.span.end))
        })
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
        .filter(|entry| {
            (!filter_module_record
                || module_statement_spans
                    .contains(&(entry.statement_span.start, entry.statement_span.end)))
                && demanded_exports.is_none_or(|demanded| match &entry.export_name {
                    ExportExportName::Name(name) => demanded.contains(name.name.as_str()),
                    ExportExportName::Default(_) => demanded.contains("default"),
                    ExportExportName::Null => true,
                })
        })
        .map(|entry| lower_export(entry, &parsed.module_record.import_entries))
        .collect::<Vec<_>>();

    for entry in parsed
        .module_record
        .star_export_entries
        .iter()
        .filter(|entry| {
            !filter_module_record
                || module_statement_spans
                    .contains(&(entry.statement_span.start, entry.statement_span.end))
        })
    {
        exports.push(lower_export(entry, &parsed.module_record.import_entries));
    }
    // When deduplicating exports, prefer runtime value exports over type-only exports (e.g.
    // when TypeScript code declares both `export type X = ...` and `export const X = ...`).
    exports.sort_by_key(|export| (export.type_only, export.span.start));
    let mut seen_exports = HashSet::new();
    exports.retain(|export| {
        if let Some(exported) = &export.exported {
            seen_exports.insert(exported.as_str().to_owned())
        } else {
            true
        }
    });
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
    let mut lowerer = Lowerer {
        text: source,
        path,
        native_import_meta,
    };
    let body = lowerer.statements(&parsed.program.body);

    let module = Module {
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
        type_syntax: HirTypeSyntax::default(),
        panicked: parsed.panicked,
        node_runtime_requests: Vec::new(),
    };
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
    /// Native-binary `import.meta` mode: `None` lowers to the standard static object with
    /// build-time paths; `Some(is_entry)` lowers to a `__kilnNativeImportMeta(isEntry)` call
    /// whose values resolve at runtime from `process.execPath` (Node SEA semantics).
    native_import_meta: Option<bool>,
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
            Statement::VariableDeclaration(declaration) => {
                if declaration.declare {
                    return None;
                }
                self.variable(declaration)
            }
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
                    if declaration.body.is_none() || declaration.declare {
                        return None;
                    }
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
                    if declaration.declare {
                        return None;
                    }
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
            | Statement::TSExternalModuleDeclaration(_)
            | Statement::TSNamespaceExportDeclaration(_) => return None,
            Statement::TSNamespaceDeclaration(declaration) => {
                if declaration.declare {
                    return None;
                }
                self.unsupported_statement("TypeScript runtime declaration", statement_span)
            }
            Statement::TSImportEqualsDeclaration(_) => {
                self.unsupported_statement("TypeScript import equals", statement_span)
            }

            Statement::FunctionDeclaration(declaration) => {
                if declaration.body.is_none() || declaration.declare {
                    return None;
                }
                self.function_declaration(declaration, statement_span)
            }
            Statement::ClassDeclaration(declaration) => {
                if declaration.declare {
                    return None;
                }
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
            Declaration::VariableDeclaration(declaration) => {
                if declaration.declare {
                    return None;
                }
                self.variable(declaration)
            }
            Declaration::TSTypeAliasDeclaration(_)
            | Declaration::TSInterfaceDeclaration(_)
            | Declaration::TSGlobalDeclaration(_) => return None,
            // `export function f() {}` is a declaration that also names an export. The export
            // side is read from the module record separately, so lowering only has to produce
            // the declaration -- the same one the unexported form produces.
            Declaration::FunctionDeclaration(declaration) => {
                if declaration.body.is_none() || declaration.declare {
                    return None;
                }
                self.function_declaration(declaration, statement_span)
            }
            Declaration::ClassDeclaration(declaration) => {
                if declaration.declare {
                    return None;
                }
                match self.class(declaration, statement_span) {
                    Ok(class) => HirStatementKind::Class(Box::new(class)),
                    Err(feature) => self.unsupported_statement(&feature, statement_span),
                }
            }
            Declaration::TSNamespaceDeclaration(declaration) => {
                if declaration.declare {
                    return None;
                }
                self.unsupported_statement("TypeScript runtime declaration", statement_span)
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
                    name_anonymous_function(value, &StringUnits::from(name.as_str()));
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
            Expression::StringLiteral(literal) => HirExpressionKind::Literal(HirLiteral::String(
                oxc_string_units(literal.value.as_str(), literal.lone_surrogates),
            )),
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
                            member.property.name.to_string().into(),
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
                // To avoid overflowing the stack on deep left-skewed binary expressions
                // (e.g. "a" + "b" + "c" + ... with hundreds or thousands of terms),
                // iteratively peel the left spine.
                let mut spine = Vec::new();
                let mut current = expression;
                loop {
                    let current_span = span(current.span);
                    let Some(operator) = lower_binary_operator(current.operator) else {
                        return self.unsupported_expression(
                            &format!("binary operator `{}`", current.operator.as_str()),
                            current_span,
                        );
                    };
                    spine.push((current_span, operator, &current.right));
                    if let Expression::BinaryExpression(left_bin) = &current.left {
                        current = left_bin;
                    } else {
                        break;
                    }
                }
                let mut acc = self.expression(&current.left);
                for (op_span, operator, right_expr) in spine.into_iter().rev() {
                    let right = self.expression(right_expr);
                    acc = HirExpression {
                        kind: HirExpressionKind::Binary {
                            operator,
                            left: Box::new(acc),
                            right: Box::new(right),
                        },
                        span: op_span,
                    };
                }
                return acc;
            }
            Expression::LogicalExpression(expression) => {
                // To avoid overflowing the stack on deep left-skewed logical expressions,
                // iteratively peel the left spine.
                let mut spine = Vec::new();
                let mut current = expression;
                loop {
                    let current_span = span(current.span);
                    let operator = match current.operator {
                        LogicalOperator::And => HirLogicalOperator::And,
                        LogicalOperator::Or => HirLogicalOperator::Or,
                        LogicalOperator::Coalesce => HirLogicalOperator::NullishCoalesce,
                    };
                    spine.push((current_span, operator, &current.right));
                    if let Expression::LogicalExpression(left_log) = &current.left {
                        current = left_log;
                    } else {
                        break;
                    }
                }
                let mut acc = self.expression(&current.left);
                for (op_span, operator, right_expr) in spine.into_iter().rev() {
                    let right = self.expression(right_expr);
                    acc = HirExpression {
                        kind: HirExpressionKind::Logical {
                            operator,
                            left: Box::new(acc),
                            right: Box::new(right),
                        },
                        span: op_span,
                    };
                }
                return acc;
            }
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
                    name_anonymous_function(
                        &mut value,
                        &StringUnits::from(identifier.name.as_str()),
                    );
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
                    property: member.property.name.to_string().into(),
                }
            }
            Expression::StaticMemberExpression(member) => HirExpressionKind::StaticMember {
                object: Box::new(self.expression(&member.object)),
                property: member.property.name.to_string().into(),
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
                        quasi.value.cooked.as_ref().map_or_else(
                            || StringUnits::from(""),
                            |text| oxc_string_units(text.as_str(), quasi.lone_surrogates),
                        )
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
                                quasi.value.cooked.as_ref().map(|text| {
                                    oxc_string_units(text.as_str(), quasi.lone_surrogates)
                                })
                            })
                            .collect(),
                        raw: tagged
                            .quasi
                            .quasis
                            .iter()
                            .map(|quasi| StringUnits::from(quasi.value.raw.as_str()))
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
                        kiln_hir::ObjectKey::Static(member.property.name.to_string().into()),
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
                // Native binary: `import.meta` resolves at runtime from `process.execPath`
                // (Node SEA semantics). The bootstrap defines `__kilnNativeImportMeta`.
                if let Some(is_entry) = self.native_import_meta {
                    HirExpressionKind::Call {
                        callee: Box::new(HirExpression {
                            kind: HirExpressionKind::Identifier(
                                "__kilnNativeImportMeta".to_string(),
                            ),
                            span: expression_span,
                        }),
                        arguments: vec![kiln_hir::ListElement::Item(HirExpression {
                            kind: HirExpressionKind::Literal(HirLiteral::Boolean(is_entry)),
                            span: expression_span,
                        })],
                        optional: false,
                    }
                } else {
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
                                key: kiln_hir::ObjectKey::Static("url".to_string().into()),
                                value: HirExpression {
                                    kind: HirExpressionKind::Literal(HirLiteral::String(
                                        url.into(),
                                    )),
                                    span: expression_span,
                                },
                                form: kiln_hir::PropertyForm::KeyValue,
                                span: expression_span,
                            }),
                            kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                                key: kiln_hir::ObjectKey::Static("dirname".to_string().into()),
                                value: HirExpression {
                                    kind: HirExpressionKind::Literal(HirLiteral::String(
                                        dirname.into(),
                                    )),
                                    span: expression_span,
                                },
                                form: kiln_hir::PropertyForm::KeyValue,
                                span: expression_span,
                            }),
                            kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                                key: kiln_hir::ObjectKey::Static("filename".to_string().into()),
                                value: HirExpression {
                                    kind: HirExpressionKind::Literal(HirLiteral::String(
                                        filename.into(),
                                    )),
                                    span: expression_span,
                                },
                                form: kiln_hir::PropertyForm::KeyValue,
                                span: expression_span,
                            }),
                        ],
                    }
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
                    property: member.property.name.to_string().into(),
                    value: Box::new(value),
                }
            }
            AssignmentTarget::StaticMemberExpression(member) if !member.optional => {
                HirExpressionKind::MemberAssignment {
                    operator,
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string().into(),
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
                                key: kiln_hir::PatternKey::Static(name.into()),
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
                                kiln_hir::PatternKey::Static(oxc_static_key_units(&named.name)?)
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
                    property: member.property.name.to_string().into(),
                })
            }
            AssignmentTarget::StaticMemberExpression(member) if !member.optional => {
                Some(kiln_hir::Pattern::Member {
                    object: Box::new(self.expression(&member.object)),
                    property: member.property.name.to_string().into(),
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
                        let Some(key) = oxc_static_key_units(&property.key) else {
                            self.unsupported("destructuring key", span);
                            return kiln_hir::Pattern::Identifier(String::new());
                        };
                        kiln_hir::PatternKey::Static(key)
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
        Some(kiln_hir::ObjectKey::Static(oxc_static_key_units(key)?))
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
                                name_anonymous_function(
                                    value,
                                    &StringUnits::from(format!("#{name}")),
                                );
                            }
                            kiln_hir::ObjectKey::Computed(_) => {}
                        }
                    }
                    initialisers.push(kiln_hir::ClassInitialiser::Field(kiln_hir::ClassField {
                        span: HirSpan::new(property.span.start, property.span.end),
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
                        inferred_name: None,
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
                    let Some(key) = oxc_static_key_units(&accessor.key) else {
                        return Err("class accessor property name".into());
                    };
                    let key = key;
                    // The backing slot is a private name the program cannot have written: `%`
                    // is not an identifier character, so this can never collide with a `#x` the
                    // class declares, and nothing can reach the storage except the two
                    // functions below.
                    let storage =
                        format!("%accessor:{}:{}", accessor.span.start, accessor.span.end);
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
                                inferred_name: None,
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
                        span: member_span,
                        key: kiln_hir::ObjectKey::Private(storage),
                        value,
                        is_static: accessor.r#static,
                    }));
                    methods.push(kiln_hir::ClassMethod {
                        span: member_span,
                        key: kiln_hir::ObjectKey::Static(key.clone().into()),
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
                        span: member_span,
                        key: kiln_hir::ObjectKey::Static(key.into()),
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
                    continue;
                }
            };
            if method.value.body.is_none() {
                continue;
            }
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
                        span: HirSpan::new(method.span.start, method.span.end),
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
            inferred_name: None,
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
            inferred_name: None,
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
                    property: member.property.name.to_string().into(),
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
                    match oxc_static_key_units(&property.key) {
                        Some(key) => kiln_hir::ObjectKey::Static(key),
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
                let Some(key) = oxc_static_key_units(&property.key) else {
                    return self.unsupported_expression("object key", expression_span);
                };
                kiln_hir::ObjectKey::Static(key)
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
                && !(name == &StringUnits::from("__proto__")
                    && form == kiln_hir::PropertyForm::KeyValue)
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

fn lower_export(
    entry: &OxcExportEntry<'_>,
    import_entries: &[oxc_syntax::module_record::ImportEntry<'_>],
) -> Export {
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
        (Some(source), ExportImportName::Name(imported), _) => {
            // Oxc's ExportImportName enum lacks a Default variant, so when an imported default
            // binding is re-exported via `export { Diff }`, Oxc sets `import_name: Name("Diff")`.
            // Check import_entries to see if this local name was actually imported from `source`
            // as Default, and restore the true ESM semantics.
            let imported_name = if import_entries.iter().any(|i| {
                i.module_request.name == source
                    && i.local_name.name == imported.name
                    && matches!(
                        i.import_name,
                        oxc_syntax::module_record::ImportImportName::Default(_)
                    )
            }) {
                ImportName::Default
            } else {
                ImportName::Name(imported.name.to_string())
            };
            ExportOrigin::ReExport {
                source,
                imported: imported_name,
            }
        }
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
fn name_anonymous_function(value: &mut kiln_hir::Expression, name: &StringUnits) {
    match &mut value.kind {
        kiln_hir::ExpressionKind::Function(function)
            if function.name.is_none() && function.inferred_name.is_none() =>
        {
            function.inferred_name = Some(name.clone());
        }
        kiln_hir::ExpressionKind::Class(class)
            if class.name.is_none() && class.inferred_name.is_none() =>
        {
            class.inferred_name = Some(name.clone());
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
        name_anonymous_function(default, &StringUnits::from(name.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_isolate_calls_do_not_claim_graph_ownership() {
        // Neither the spelling of a callee nor a local alias proves that a JS object graph
        // can leave its realm. In particular, a user function and an arbitrary object's
        // method named `isolate` have no relationship to the future transfer built-in.
        for source in [
            "function isolate(value) { return value; } const value = {}; const alias = value; isolate(value); alias.x = 1;",
            "const helper = { isolate(value) { return value; } }; const value = {}; const alias = value; helper.isolate(value); alias.x = 1;",
        ] {
            let module = parse("isolate.js", source);
            assert!(
                module.diagnostics.is_empty(),
                "{source}: {:?}",
                module.diagnostics
            );
        }
    }

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
    fn node_runtime_discovery_follows_imported_bindings_and_rejects_shadowing() {
        let source = r#"
            import { createRequire } from "node:module";
            const req = createRequire(import.meta.url);
            req("node:path");
            createRequire(import.meta.url)("node:url");
            process.getBuiltinModule("node:util");
            function shadow(createRequire) {
                createRequire(import.meta.url)("node:fs");
            }
            function local() {
                const createRequire = () => () => {};
                createRequire(import.meta.url)("node:os");
            }
        "#;
        let mut requests = discover_node_runtime_requests("fixture.mjs", source);
        requests.sort_by(|left, right| left.specifier.cmp(&right.specifier));
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.specifier)
                .collect::<Vec<_>>(),
            ["node:path", "node:url", "node:util"]
        );
    }

    #[test]
    fn node_runtime_discovery_accepts_aliased_get_builtin_module_receiver() {
        // Bundlers alias `process` (e.g. a parameter named `e`); the builtin id is still
        // statically provable, so it is still a link-time request.
        let source = r#"
            function bundle(e) {
                e.getBuiltinModule("node:os");
                e.getBuiltinModule(`node:path`);
            }
            process.getBuiltinModule("node:util");
        "#;
        let mut requests = discover_node_runtime_requests("fixture.mjs", source);
        requests.sort_by(|left, right| left.specifier.cmp(&right.specifier));
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.specifier)
                .collect::<Vec<_>>(),
            ["node:os", "node:path", "node:util"]
        );
    }

    #[test]
    fn actor_module_literal_is_a_lazy_graph_request() {
        let requests = discover_node_runtime_requests(
            "app/main.mjs",
            "import { spawnActor } from 'kiln:actors';\n\
             spawnActor({ module: new URL('./counter.mjs', import.meta.url), export: 'Counter' });",
        );
        assert!(requests.iter().any(|request| {
            request.specifier == "./counter.mjs" && request.importer == "app/main.mjs"
        }));
    }

    #[test]
    fn node_runtime_discovery_follows_commonjs_module_alias() {
        let source = r#"
            const m = require("node:module");
            const req = m.createRequire(__filename);
            req("node:path");
            m.createRequire(__filename)("node:url");
            function shadow(m) {
                m.createRequire(__filename)("node:fs");
            }
        "#;
        let mut requests = discover_node_runtime_requests("fixture.cjs", source);
        requests.sort_by(|left, right| left.specifier.cmp(&right.specifier));
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.specifier)
                .collect::<Vec<_>>(),
            ["node:path", "node:url"]
        );
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
    fn dce_roots_only_demanded_direct_exports() {
        let demanded = BTreeSet::from(["keep".to_owned()]);
        let module = parse_dce(
            "module.js",
            r#"
                export function keep() { return 1 }
                export function drop() { return 2 }
                export const effect = globalThis.observe()
            "#,
            &demanded,
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["keep"]);
        assert!(module.body.iter().any(|statement| {
            matches!(
                &statement.kind,
                HirStatementKind::Function(function)
                    if function.name.as_deref() == Some("keep")
            )
        }));
        assert!(!module.body.iter().any(|statement| {
            matches!(
                &statement.kind,
                HirStatementKind::Function(function)
                    if function.name.as_deref() == Some("drop")
            )
        }));
        assert!(
            module.body.len() > 1,
            "the undemanded but effectful initializer must remain"
        );
    }

    #[test]
    fn dce_roots_only_demanded_named_export_aliases() {
        let module = parse_dce(
            "module.js",
            "const keep = 1; const drop = 2; export { keep as answer, drop as excess };",
            &BTreeSet::from(["answer".to_owned()]),
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["answer"]);
        let declares = |name: &str| {
            module.body.iter().any(|statement| match &statement.kind {
                HirStatementKind::Variable { declarations, .. } => declarations.iter().any(
                    |declaration| matches!(&declaration.pattern, kiln_hir::Pattern::Identifier(found) if found == name),
                ),
                _ => false,
            })
        };
        assert!(declares("keep"));
        assert!(!declares("drop"));
    }

    #[test]
    fn dce_preserves_inherited_class_evaluation() {
        let module = parse_dce(
            "module.js",
            "class Unused extends Later {} class Later {} console.log('continued')",
            &BTreeSet::new(),
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.body.len(), 3);
    }

    /// The HIR of a release (DCE) parse, as text: enough to ask whether a name survived.
    fn dce_text(source: &str) -> String {
        let module = parse_dce("module.js", source, &BTreeSet::new());
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let text = format!("{:?}", module.body);
        assert!(
            !text.contains(dce_guard::OBSERVABLE),
            "a DCE guard survived into the HIR"
        );
        text
    }

    #[test]
    fn dce_keeps_expressions_whose_coercion_can_run_user_code() {
        // Each statement's only effect is a coercion of `o` (issue 19); Oxc assumes coercions
        // are pure. `k` is a parameter, so it survives as a use only if the statement does.
        for source in [
            "function f(o, k) { o < k; } f({}, 1);",
            "function f(o, k) { o == k; } f({}, 1);",
            "function f(o, k) { `${o}${k}`; } f({}, 1);",
            "function f(o, k) { [o] + k; } f({}, 1);",
            "function f(o, k) { ({ [o]: k }); } f({}, 1);",
            "function f(o, k) { class C { [o]() { return k; } } } f({}, 1);",
            "function f(o, k) { const unused = o < k; } f({}, 1);",
            "function f(o, k) { String(o, k); } f({}, 1);",
            "function f(o, k) { Math.abs(o, k); } f({}, 1);",
            "function f(o, k) { new Date(o, k); } f({}, 1);",
        ] {
            let text = dce_text(source);
            assert!(text.matches("\"k\"").count() >= 2, "{source}: {text}");
        }
        for source in [
            "function f(o) { +o; } f({});",
            "function f(o) { -o; } f({});",
            "function f(o) { ~o; } f({});",
        ] {
            assert!(dce_text(source).contains("Unary"), "{source}");
        }
    }

    #[test]
    fn dce_keeps_calls_and_classes_that_may_throw() {
        for (source, marker) in [
            (
                "try { decodeURIComponent('%'); } catch {}",
                "decodeURIComponent",
            ),
            ("try { decodeURI?.('%'); } catch {}", "decodeURI"),
            ("try { parseInt('1'); } catch {}", "parseInt"),
            (
                "function f() { const n = 1; class KeptSubclass extends n {} } f();",
                "KeptSubclass",
            ),
        ] {
            assert!(dce_text(source).contains(marker), "{source}");
        }
    }

    #[test]
    fn dce_guard_keeps_what_the_pass_is_for() {
        // Primitive comparisons still fold, `x == null` and `===` do not coerce, unused
        // declarations still go, and module-scope allocations bundles never use still go.
        let text = dce_text(
            "const LIMIT = 3; if (LIMIT > 5) { deadBranch(); } \
             function f(x) { x == null; x === 1; } f(1); \
             function unusedFunction() {} \
             const unusedMap = new Map(); const unusedSymbol = Symbol('s'); \
             function g() { class UnusedBase {} class UnusedDerived extends UnusedBase {} } g(); \
             console.log('live');",
        );
        for dropped in [
            "deadBranch",
            "unusedFunction",
            "unusedMap",
            "unusedSymbol",
            "UnusedDerived",
        ] {
            assert!(!text.contains(dropped), "{dropped} survived: {text}");
        }
    }

    #[test]
    fn dce_shakes_neighbors_of_an_exported_inherited_class() {
        let module = parse_dce(
            "module.js",
            "export class Kept extends Base {} export function dropped() {}",
            &BTreeSet::new(),
        );

        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert!(module.body.iter().any(|statement| matches!(
            &statement.kind,
            HirStatementKind::Class(class) if class.name.as_deref() == Some("Kept")
        )));
        assert!(!module.body.iter().any(|statement| matches!(
            &statement.kind,
            HirStatementKind::Function(function) if function.name.as_deref() == Some("dropped")
        )));
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
    fn preserves_runtime_export_when_type_and_value_share_identifier() {
        let module = parse(
            "module.ts",
            r#"
                export type Result<T> = { ok: true; value: T };
                export const Result = {
                    ok: (v) => ({ ok: true, value: v }),
                };
            "#,
        );

        assert_eq!(module.source_kind, SourceKind::TypeScript);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        assert_eq!(module.runtime_export_names(), ["Result"]);
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
    fn lightweight_module_specifiers_cover_static_and_typescript_requests() {
        let source = r#"
            /// <reference types="@types/pkg" />
            import type { Shape } from "pkg\x2fstatic";
            export type { Shape as ExportedShape } from "pkg\u002fexport";
            type Imported = import("pkg\x2ftype").Shape;
            import Alias = require("pkg\x2fequals");
        "#;
        let module = parse("module.ts", source);
        let found = discover_module_specifiers("module.ts", source);
        let expected_static = module
            .requested_modules
            .iter()
            .map(|request| request.specifier.as_str())
            .collect::<BTreeSet<_>>();
        assert!(
            expected_static
                .iter()
                .all(|specifier| found.iter().any(|found| found == specifier)),
            "static module-record requests must be retained: {found:?}"
        );
        for expected in [
            "pkg/static",
            "pkg/export",
            "pkg/type",
            "pkg/equals",
            "@types/pkg",
        ] {
            assert!(
                found.iter().any(|specifier| specifier == expected),
                "missing decoded module specifier {expected:?} from {found:?}"
            );
        }
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
    fn retained_type_declarations_stay_in_function_and_namespace_scopes() {
        let only_alias = parse("only.ts", "type Only = number;");
        let only = only_alias
            .type_syntax
            .declarations
            .iter()
            .find(|d| d.name.as_deref() == Some("Only"))
            .unwrap();
        assert!(
            only_alias
                .type_syntax
                .body
                .iter()
                .any(|statement| statement.span == only.span)
        );

        let module = parse(
            "scopes.ts",
            r#"
function outer() {
    type Local = string;
    return () => { type Nested = Local; return "ok"; };
}
namespace Outer {
    type Local = number;
    namespace Inner { type Nested = Local; }
}
"#,
        );
        let declaration = |name: &str| {
            module
                .type_syntax
                .declarations
                .iter()
                .find(|d| d.name.as_deref() == Some(name))
                .unwrap()
        };
        let local = declaration("Local");
        let outer_function = module
            .type_syntax
            .body
            .iter()
            .find_map(|statement| match &statement.kind {
                HirStatementKind::Function(function)
                    if function.name.as_deref() == Some("outer") =>
                {
                    Some(function)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            outer_function
                .body
                .iter()
                .any(|statement| statement.span == local.span)
        );
        let nested = declaration("Nested");
        let arrow = outer_function
            .body
            .iter()
            .find_map(|statement| match &statement.kind {
                HirStatementKind::Return(Some(expression)) => match &expression.kind {
                    HirExpressionKind::Function(function) => Some(function),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert!(
            arrow
                .body
                .iter()
                .any(|statement| statement.span == nested.span)
        );

        let namespace = declaration("Outer");
        let namespace_local = module
            .type_syntax
            .declarations
            .iter()
            .find(|d| d.name.as_deref() == Some("Local") && d.span.start > namespace.span.start)
            .unwrap();
        assert!(
            namespace
                .body
                .iter()
                .any(|statement| statement.span == namespace_local.span)
        );
        let inner = module
            .type_syntax
            .declarations
            .iter()
            .find(|d| d.name.as_deref() == Some("Inner"))
            .unwrap();
        let nested_type = module
            .type_syntax
            .declarations
            .iter()
            .find(|d| d.name.as_deref() == Some("Nested") && d.span.start > inner.span.start)
            .unwrap();
        assert!(
            inner
                .body
                .iter()
                .any(|statement| statement.span == nested_type.span)
        );
    }

    #[test]
    fn export_modifiers_apply_only_to_the_direct_declaration() {
        let module = parse(
            "exports.ts",
            r#"
export function outer() {
    type Local = number;
    class Inner {}
    return 1;
}
export namespace Boxed {
    type Hidden = string;
    export type Visible = number;
}
export default interface Fallback { value: boolean }
"#,
        );

        let declaration = |name: &str| {
            module
                .type_syntax
                .declarations
                .iter()
                .find(|declaration| declaration.name.as_deref() == Some(name))
                .unwrap()
        };
        let outer = module
            .type_syntax
            .signatures
            .iter()
            .find(|signature| signature.has_body)
            .unwrap();
        assert!(
            module
                .type_syntax
                .exported_declarations
                .contains(&outer.owner_span)
        );
        assert!(!declaration("Local").exported);
        assert!(
            !module
                .type_syntax
                .classes
                .iter()
                .find(|class| class.name.as_deref() == Some("Inner"))
                .unwrap()
                .exported
        );

        assert!(declaration("Boxed").exported);
        assert!(!declaration("Hidden").exported);
        assert!(declaration("Visible").exported);

        assert!(declaration("Fallback").default_exported);
    }

    #[test]
    fn type_signatures_retain_destructured_and_rest_parameter_patterns() {
        let module = parse(
            "signature-patterns.d.ts",
            "declare function consume({ value }: { value: number }, ...rest: number[]): void;",
        );
        let signature = module
            .type_syntax
            .signatures
            .iter()
            .find(|site| !site.has_body)
            .unwrap();
        assert!(matches!(
            signature.signature.parameters[0].pattern.as_deref(),
            Some(kiln_hir::Pattern::Object { .. })
        ));
        assert_eq!(
            signature.signature.parameters[1].pattern.as_deref(),
            Some(&kiln_hir::Pattern::Identifier("rest".to_owned()))
        );
        let rest_span = signature.signature.parameters[1].pattern_span;
        assert_eq!(
            module
                .source
                .get(rest_span.start as usize..rest_span.end as usize),
            Some("rest")
        );
    }

    #[test]
    fn type_names_use_decoded_oxc_identifiers_instead_of_source_spelling() {
        let module = parse(
            "escaped-types.ts",
            r#"type Exported\u0042ase = string;
type Ref = Exported\u0042ase;
type Query = typeof Exported\u0042ase;"#,
        );
        let alias = module
            .type_syntax
            .declarations
            .iter()
            .find(|declaration| declaration.name.as_deref() == Some("Ref"))
            .unwrap();
        assert!(matches!(
            &alias.kind,
            kiln_hir::TypeDeclarationKind::Alias(kiln_hir::TypeExpr {
                kind: kiln_hir::TypeExprKind::Reference { name, .. },
                ..
            }) if name == &["ExportedBase"]
        ));

        let query = module
            .type_syntax
            .declarations
            .iter()
            .find(|declaration| declaration.name.as_deref() == Some("Query"))
            .unwrap();
        assert!(matches!(
            &query.kind,
            kiln_hir::TypeDeclarationKind::Alias(kiln_hir::TypeExpr {
                kind: kiln_hir::TypeExprKind::Typeof { name, .. },
                ..
            }) if name == &["ExportedBase"]
        ));
    }

    #[test]
    fn import_type_options_are_owned_for_import_and_typeof_import_forms() {
        let module = parse(
            "import-type-options.ts",
            r#"type Direct = import("pkg", { with: { "resolution-mode": "require" } }).Entry;
type Queried = typeof import("pkg", { with: { "resolution-mode": "require" } }).Entry;"#,
        );
        let options_for = |name: &str| {
            let declaration = module
                .type_syntax
                .declarations
                .iter()
                .find(|declaration| declaration.name.as_deref() == Some(name))
                .unwrap();
            let kiln_hir::TypeDeclarationKind::Alias(ty) = &declaration.kind else {
                panic!("expected alias type for {name}");
            };
            let kiln_hir::TypeExprKind::Import {
                options: Some(options),
                ..
            } = &ty.kind
            else {
                panic!("expected preserved import options for {name}");
            };
            options
        };
        for name in ["Direct", "Queried"] {
            let options = options_for(name);
            assert!(matches!(
                &options.kind,
                kiln_hir::ExpressionKind::Object { .. }
            ));
            let source = module
                .source
                .get(options.span.start as usize..options.span.end as usize)
                .unwrap();
            assert!(source.contains("resolution-mode"), "{source}");
            assert!(source.contains("require"), "{source}");
        }
    }

    #[test]
    fn static_module_attributes_retain_keyword_key_value_and_spans() {
        let module = parse(
            "module-attributes.ts",
            r#"import value from "with-package" with /* keyword trivia */ { "resolution-mode": "require" };
export { value } from "pkg" with { "resolution-mode": "invalid-mode" };
export * from "legacy" assert { type: "json" };"#,
        );
        assert_eq!(module.type_syntax.module_attributes.len(), 3);
        let with_import = &module.type_syntax.module_attributes[0];
        assert_eq!(
            with_import.keyword,
            kiln_hir::TypeModuleAttributesKeyword::With
        );
        assert_eq!(with_import.attributes.len(), 1);
        assert_eq!(with_import.attributes[0].key, "resolution-mode");
        assert_eq!(with_import.attributes[0].value, "require");
        assert_eq!(
            with_import.attributes[0].key_kind,
            kiln_hir::TypeModuleAttributeKeyKind::StringLiteral
        );
        assert_eq!(
            module.source.get(
                with_import.keyword_span.start as usize..with_import.keyword_span.end as usize
            ),
            Some("with")
        );
        let attributes_span = with_import.attributes_span;
        assert_eq!(
            module
                .source
                .get(attributes_span.start as usize..attributes_span.end as usize),
            Some(r#"{ "resolution-mode": "require" }"#)
        );

        let with_export = &module.type_syntax.module_attributes[1];
        assert_eq!(with_export.attributes[0].value, "invalid-mode");
        let assert_export = &module.type_syntax.module_attributes[2];
        assert_eq!(
            assert_export.keyword,
            kiln_hir::TypeModuleAttributesKeyword::Assert
        );
        assert_eq!(assert_export.attributes[0].key, "type");
        assert_eq!(assert_export.attributes[0].value, "json");
        assert_eq!(
            module.source.get(
                assert_export.keyword_span.start as usize..assert_export.keyword_span.end as usize
            ),
            Some("assert")
        );

        let javascript = parse(
            "attrs.mjs",
            "import value from 'pkg' with { type: 'json' }; function f(x) { return x; }",
        );
        assert_eq!(javascript.type_syntax.module_attributes.len(), 1);
        assert!(javascript.type_syntax.signatures.is_empty());
        assert!(javascript.type_syntax.body.is_empty());
    }

    #[test]
    fn captures_triple_slash_reference_directives_from_oxc_comments() {
        let module = parse(
            "refs.d.ts",
            "/// <reference path=\"./base.d.ts\" />\n/// <reference types=\"node\" resolution-mode=\"require\" />\n/// <reference lib=\"esnext\" />\n/// <reference no-default-lib=\"true\" />\ninterface X {}\n",
        );
        assert_eq!(module.type_syntax.references.len(), 3);
        assert_eq!(
            module.type_syntax.references[0].kind,
            kiln_hir::TypeReferenceKind::Path
        );
        assert_eq!(module.type_syntax.references[0].value, "./base.d.ts");
        assert_eq!(
            module.type_syntax.references[1].resolution_mode,
            Some(kiln_hir::TypeResolutionMode::Require)
        );
        assert_eq!(
            module.type_syntax.references[2].kind,
            kiln_hir::TypeReferenceKind::Lib
        );
        assert!(module.type_syntax.no_default_lib);
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
                HirExpressionKind::Function(function) => {
                    Some(function.name.clone().or_else(|| {
                        function
                            .inferred_name
                            .as_ref()
                            .map(StringUnits::to_lossy_string)
                    }))
                }
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
            let display_name = function.name.clone().or_else(|| {
                function
                    .inferred_name
                    .as_ref()
                    .map(StringUnits::to_lossy_string)
            });
            assert_eq!(display_name.as_deref(), expected, "{source}");
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
        assert_eq!(class.inferred_name, Some(StringUnits::from("C")));
    }

    #[test]
    fn inferred_property_names_keep_lone_surrogate_units() {
        let module = parse_with_goal(
            "inferred-name.js",
            r#"const object = { "\ud800": function () {}, "\udc00": () => {}, "\ufffd": function () {} };"#,
            SourceGoal::Script,
        );
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let HirStatementKind::Variable { declarations, .. } = &module.body[0].kind else {
            panic!("expected a variable declaration");
        };
        let Some(initializer) = &declarations[0].initializer else {
            panic!("expected an object initializer");
        };
        let HirExpressionKind::Object { members } = &initializer.kind else {
            panic!("expected an object initializer, got {:?}", initializer.kind);
        };
        let mut found = Vec::new();
        for member in members {
            let kiln_hir::ObjectMember::Property(property) = member else {
                continue;
            };
            let kiln_hir::ObjectKey::Static(key) = &property.key else {
                continue;
            };
            let HirExpressionKind::Function(function) = &property.value.kind else {
                panic!("expected a function value");
            };
            found.push((key.to_units().to_vec(), function.inferred_name.clone()));
        }
        assert_eq!(
            found,
            vec![
                (vec![0xd800], Some(StringUnits::from_units(&[0xd800]))),
                (vec![0xdc00], Some(StringUnits::from_units(&[0xdc00]))),
                (vec![0xfffd], Some(StringUnits::from_units(&[0xfffd]))),
            ]
        );
    }

    #[test]
    fn auto_accessor_storage_identity_does_not_use_lossy_key_text() {
        let module = parse_with_goal(
            "accessor-name.js",
            r#"class Holder { accessor "\ud800" = 1; accessor "\ufffd" = 2; }"#,
            SourceGoal::Script,
        );
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let HirStatementKind::Class(class) = &module.body[0].kind else {
            panic!("expected a class declaration");
        };
        let storage: Vec<_> = class
            .initialisers
            .iter()
            .filter_map(|initializer| initializer.as_field())
            .filter_map(|field| match &field.key {
                kiln_hir::ObjectKey::Private(name) => Some(name.clone()),
                kiln_hir::ObjectKey::Static(_) | kiln_hir::ObjectKey::Computed(_) => None,
            })
            .collect();
        assert_eq!(storage.len(), 2);
        assert_ne!(storage[0], storage[1]);
        assert!(storage.iter().all(|name| name.starts_with("%accessor:")));
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
            .find(|method| matches!(method.key, kiln_hir::ObjectKey::Static(ref name) if name == &StringUnits::from("read")))
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

    #[test]
    fn lowers_oxc_lone_surrogates_and_tagged_template_raw_text_losslessly() {
        let module = parse(
            "surrogates.js",
            r#"const literal = "\ud800"; const tagged = tag`\ud800�abcd${value}`;"#,
        );
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let HirStatementKind::Variable { declarations, .. } = &module.body[0].kind else {
            panic!("expected literal declaration");
        };
        let HirExpressionKind::Literal(HirLiteral::String(literal)) = &declarations[0]
            .initializer
            .as_ref()
            .expect("literal initializer")
            .kind
        else {
            panic!("expected literal string");
        };
        assert_eq!(literal.to_units().as_ref(), &[0xd800]);

        let HirStatementKind::Variable { declarations, .. } = &module.body[1].kind else {
            panic!("expected tagged declaration");
        };
        let HirExpressionKind::Call {
            callee: _,
            arguments,
            ..
        } = &declarations[0]
            .initializer
            .as_ref()
            .expect("tagged initializer")
            .kind
        else {
            panic!("expected tagged call");
        };
        let ListElement::Item(template) = &arguments[0] else {
            panic!("expected template object argument");
        };
        let HirExpressionKind::TemplateStrings { cooked, raw } = &template.kind else {
            panic!("expected template strings");
        };
        assert_eq!(
            cooked[0].as_ref().expect("cooked").to_units().as_ref(),
            &[
                0xd800,
                0xfffd,
                b'a' as u16,
                b'b' as u16,
                b'c' as u16,
                b'd' as u16
            ]
        );
        assert_eq!(
            raw[0].to_units().as_ref(),
            "\\ud800�abcd".encode_utf16().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_ambient_function_overloads() {
        let source = r#"
declare const globalConst: string;
declare function globalFunc(): void;
declare class GlobalClass {}

export function defineService(a: number): void;
export function defineService(a: string): void;
export function defineService(a: any): void {
    return a;
}

function localOverload(x: number): void;
function localOverload(x: any): void {}

class MyClass {
    [key: string]: any;
    compute(x: number): number;
    compute(x: string): string;
    compute(x: any): any {
        return x;
    }
}
"#;
        let module = parse("overloads.ts", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        for stmt in &module.body {
            assert!(
                !matches!(stmt.kind, kiln_hir::StatementKind::Unsupported { .. }),
                "found unsupported statement: {:?}",
                stmt.kind
            );
        }
        // There should be exactly 3 statements: defineService, localOverload, and MyClass.
        assert_eq!(module.body.len(), 3);
        // Exports should contain exactly 1 entry for defineService.
        let define_service_exports: Vec<_> = module
            .exports
            .iter()
            .filter(|e| e.exported.as_ref().map(|n| n.as_str()) == Some("defineService"))
            .collect();
        assert_eq!(define_service_exports.len(), 1);
    }

    #[test]
    fn test_import_default_and_export_named() {
        let source = r#"
import Diff from './diff/base.js';
export { Diff };
"#;
        let module = parse("index.js", source);
        assert!(module.diagnostics.is_empty(), "{:?}", module.diagnostics);
        let diff_export = module
            .exports
            .iter()
            .find(|e| e.exported.as_ref().map(|n| n.as_str()) == Some("Diff"))
            .expect("Diff export");
        assert_eq!(
            diff_export.origin,
            ExportOrigin::ReExport {
                source: "./diff/base.js".to_owned(),
                imported: ImportName::Default,
            }
        );
    }
}

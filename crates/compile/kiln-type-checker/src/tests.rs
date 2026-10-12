use std::sync::{Arc, OnceLock};

use crate::{
    CheckOptions, CheckResult, FactSelection, SourceDiagnostic, SourceSnapshot, SourceType,
    TypeClass, TypeRole, check,
};

const LIB_ROOT: &str = "/__kiln_typescript_lib";
const PROJECT_ROOT: &str = "/project";

fn parsed_libraries() -> &'static (SourceSnapshot, Vec<String>) {
    static LIBRARIES: OnceLock<(SourceSnapshot, Vec<String>)> = OnceLock::new();
    LIBRARIES.get_or_init(|| {
        let mut snapshot = SourceSnapshot::default();
        let mut roots = Vec::new();
        for (name, source) in crate::STANDARD_LIBRARIES {
            let path = format!("{LIB_ROOT}/{name}");
            snapshot.modules.insert(
                path.clone(),
                Arc::new(kiln_frontend_oxc::parse(&path, source)),
            );
            roots.push(path);
        }
        (snapshot, roots)
    })
}

fn options() -> CheckOptions {
    CheckOptions {
        libraries: vec!["es5".into()],
        current_directory: PROJECT_ROOT.into(),
        ..CheckOptions::default()
    }
}

fn snapshot_with(sources: &[(&str, &str)]) -> (SourceSnapshot, Vec<String>) {
    let (base, library_roots) = parsed_libraries();
    let mut snapshot = base.clone();
    let mut roots = Vec::with_capacity(sources.len() + library_roots.len());
    for (path, source) in sources {
        snapshot.modules.insert(
            (*path).to_owned(),
            Arc::new(kiln_frontend_oxc::parse(path, source)),
        );
        roots.push((*path).to_owned());
    }
    // Standard libraries are loaded by options and their owned reference directives.
    let _ = library_roots;
    (snapshot, roots)
}

fn check_sources(sources: &[(&str, &str)]) -> CheckResult {
    check_sources_with_options(sources, &options())
}

fn check_sources_with_options(sources: &[(&str, &str)], options: &CheckOptions) -> CheckResult {
    let (snapshot, roots) = snapshot_with(sources);
    check(&snapshot, &roots, options)
}

fn check_snapshot_with_options(
    snapshot: &SourceSnapshot,
    roots: &[String],
    options: &CheckOptions,
) -> CheckResult {
    check(snapshot, roots, options)
}

fn assert_complete(result: &CheckResult) {
    assert!(
        result.complete,
        "checker result was incomplete; adapter gaps must remain visible: {}",
        format_diagnostics(&result.diagnostics)
    );
}

fn assert_clean(result: &CheckResult) {
    assert!(
        result.diagnostics.is_empty(),
        "expected a clean type check: {}",
        format_diagnostics(&result.diagnostics)
    );
}

fn format_diagnostics(diagnostics: &[SourceDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| format!("{:?} TS{:?} {:?}: {}", d.path, d.code, d.span, d.message))
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_code(result: &CheckResult, code: u32) -> bool {
    result.diagnostics.iter().any(|d| d.code == Some(code))
}

fn types_for<'a>(result: &'a CheckResult, path: &str, role: &TypeRole) -> Vec<&'a SourceType> {
    result
        .types
        .iter()
        .filter(|ty| ty.path == path && &ty.role == role)
        .collect()
}

fn expression_type<'a>(
    result: &'a CheckResult,
    path: &str,
    source: &str,
    expression: &str,
) -> &'a SourceType {
    let expected_start = source
        .find(expression)
        .expect("expression appears in fixture") as u32;
    let expected_end = expected_start + expression.len() as u32;
    types_for(result, path, &TypeRole::Expression)
        .into_iter()
        .find(|ty| ty.span.start == expected_start && ty.span.end == expected_end)
        .unwrap_or_else(|| panic!("no expression type fact for `{expression}`"))
}

#[test]
fn reports_bun_engine_negative_annotation_as_ts2322() {
    let path = "/project/main.ts";
    let source = "const count: number = \"not a number\";";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert!(
        has_code(&result, 2322),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn checker_marks_unrepresentable_generated_capability_expressions_incomplete() {
    let path = "/project/synthetic.ts";
    let source = "const value = 1;                    ";
    let (mut snapshot, roots) = snapshot_with(&[(path, source)]);
    let mut module = kiln_frontend_oxc::parse_static_typescript(
        path,
        source,
        kiln_frontend_oxc::StaticTypescriptPolicy::PreserveAuthoredSource,
    );
    for (kind, start) in [
        (kiln_hir::ExpressionKind::GlobalObject, 17),
        (kiln_hir::ExpressionKind::ProcessObject, 19),
    ] {
        let span = kiln_hir::Span::new(start, start + 1);
        module.type_syntax.body.push(kiln_hir::Statement {
            kind: kiln_hir::StatementKind::Expression(kiln_hir::Expression { kind, span }),
            span,
        });
    }
    snapshot.modules.insert(path.into(), Arc::new(module));

    let result = check_snapshot_with_options(&snapshot, &roots, &options());
    assert!(
        !result.complete,
        "checker must not silently assign a type to unrepresentable compiler capabilities"
    );
    for message in [
        "compiler-generated global object expression",
        "compiler-generated process capability expression",
    ] {
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(message)),
            "missing explicit adapter gap {message:?}: {}",
            format_diagnostics(&result.diagnostics)
        );
    }
}

#[test]
fn checker_treats_inline_literal_eval_as_its_original_call() {
    let path = "/project/eval.ts";
    let source = "eval('1');";
    let (mut snapshot, roots) = snapshot_with(&[(path, source)]);
    let mut module = kiln_frontend_oxc::parse_static_typescript(
        path,
        source,
        kiln_frontend_oxc::StaticTypescriptPolicy::PreserveAuthoredSource,
    );
    let call_span = kiln_hir::Span::new(0, source.len() as u32);
    module.type_syntax.body.push(kiln_hir::Statement {
        kind: kiln_hir::StatementKind::Expression(kiln_hir::Expression {
            kind: kiln_hir::ExpressionKind::InlineLiteralEval {
                callee: Box::new(kiln_hir::Expression {
                    kind: kiln_hir::ExpressionKind::Identifier("eval".into()),
                    span: kiln_hir::Span::new(0, 4),
                }),
                argument: Box::new(kiln_hir::Expression {
                    kind: kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::String(
                        kiln_hir::StringUnits::from("1"),
                    )),
                    span: kiln_hir::Span::new(5, 8),
                }),
                expression: Box::new(kiln_hir::Expression {
                    kind: kiln_hir::ExpressionKind::Literal(kiln_hir::Literal::Number(
                        1.0f64.to_bits(),
                    )),
                    span: kiln_hir::Span::new(0, 1),
                }),
            },
            span: call_span,
        }),
        span: call_span,
    });
    snapshot.modules.insert(path.into(), Arc::new(module));

    let result = check_snapshot_with_options(&snapshot, &roots, &options());
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn es5_dom_declarations_retain_statement_modifiers_and_index_member_types() {
    let path = "/project/main.ts";
    let mut declaration_options = options();
    declaration_options.libraries = vec!["es5".into(), "dom".into()];
    let (mut snapshot, roots) = snapshot_with(&[
        (path, "const root: Node = document.documentElement;"),
        (
            "/project/modifiers.d.ts",
            "export declare const declaredValue: number; export declare function declaredFunction(): void;",
        ),
    ]);
    snapshot.auxiliary_files.insert(
        "/project/tsconfig.json".into(),
        Arc::from(br#"{"compilerOptions":{"lib":["es5","dom"],"skipLibCheck":false}}"#.as_slice()),
    );
    declaration_options.config_path = Some("/project/tsconfig.json".into());
    let result = check_snapshot_with_options(&snapshot, &roots, &declaration_options);
    assert_complete(&result);
    assert!(
        result.diagnostics.is_empty(),
        "ES5/DOM declaration libraries and `export declare` statements should check cleanly: {}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn strict_option_controls_null_assignability() {
    let path = "/project/main.ts";
    let source = "const value: string = null;";
    let strict_result = check_sources(&[(path, source)]);
    assert_complete(&strict_result);
    assert!(
        has_code(&strict_result, 2322),
        "strict null checking should reject null: {}",
        format_diagnostics(&strict_result.diagnostics)
    );

    let mut loose_options = options();
    loose_options.strict = false;
    let loose_result = check_sources_with_options(&[(path, source)], &loose_options);
    assert_complete(&loose_result);
    assert_clean(&loose_result);
}

#[test]
fn jsonc_config_facade_applies_overriding_options_from_snapshot() {
    let path = "/project/main.ts";
    let source = "const value: string = null;";
    let (mut snapshot, roots) = snapshot_with(&[(path, source)]);
    snapshot.auxiliary_files.insert(
        "/project/tsconfig.json".into(),
        Arc::from(
            br#"{
              // JSONC comments and trailing commas are accepted by tsconfig.
              "compilerOptions": { "strict": false, "skipLibCheck": true, },
            }"#
            .as_slice(),
        ),
    );
    let mut configured = options();
    configured.config_path = Some("/project/tsconfig.json".into());
    let result = check_snapshot_with_options(&snapshot, &roots, &configured);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn config_facade_inherits_path_mappings_and_child_option_overrides() {
    let path = "/project/main.ts";
    let source =
        "import { amount } from '@amount'; const copy: number = amount; const bad: string = null;";
    let (mut snapshot, roots) = snapshot_with(&[(path, source)]);
    snapshot.auxiliary_files.insert(
        "/project/base.json".into(),
        Arc::from(
            br#"{
              "compilerOptions": {
                "paths": { "@amount": ["./types.d.ts"] },
                "strict": false,
                "skipLibCheck": true,
              },
            }"#
            .as_slice(),
        ),
    );
    snapshot.auxiliary_files.insert(
        "/project/tsconfig.json".into(),
        Arc::from(
            br#"{
              "extends": "./base.json",
              "compilerOptions": { "strictNullChecks": true, },
            }"#
            .as_slice(),
        ),
    );
    snapshot.modules.insert(
        "/project/types.d.ts".into(),
        Arc::new(kiln_frontend_oxc::parse(
            "/project/types.d.ts",
            "export declare const amount: number;",
        )),
    );
    let mut configured = options();
    configured.config_path = Some("/project/tsconfig.json".into());
    let result = check_snapshot_with_options(&snapshot, &roots, &configured);
    assert_complete(&result);
    assert!(
        has_code(&result, 2322),
        "child strictNullChecks must override inherited strict=false: {}",
        format_diagnostics(&result.diagnostics)
    );
    assert!(
        !has_code(&result, 2307),
        "inherited paths mapping should resolve @amount: {}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn config_facade_reports_unknown_and_malformed_options() {
    let path = "/project/main.ts";
    let (mut snapshot, roots) = snapshot_with(&[(path, "const value = 1;")]);
    snapshot.auxiliary_files.insert(
        "/project/unknown.json".into(),
        Arc::from(br#"{"compilerOptions":{"skipLibCheck":true,"unknownOption":true}}"#.as_slice()),
    );
    snapshot.auxiliary_files.insert(
        "/project/malformed.json".into(),
        Arc::from(br#"{"compilerOptions":{"strict":true}"#.as_slice()),
    );
    let mut configured = options();
    configured.config_path = Some("/project/unknown.json".into());
    let unknown = check_snapshot_with_options(&snapshot, &roots, &configured);
    assert!(
        has_code(&unknown, 5023),
        "unknown option should report TS5023: {}",
        format_diagnostics(&unknown.diagnostics)
    );
    let valid_module =
        kiln_frontend_oxc::parse(path, "function add(value: number) { return value + 1; }");
    let mut program = kiln_ir::compile_module(&valid_module).expect("valid source compiles");
    kiln_ir::apply_source_types(
        path,
        &valid_module,
        &unknown,
        crate::CHECKER_VERSION,
        &mut program.program,
    );
    assert!(
        program.program.functions.iter().all(|function| !function
            .type_annotations
            .as_ref()
            .is_some_and(|annotations| annotations.source_types.is_some())),
        "invalid config must not publish compiler annotations"
    );

    configured.config_path = Some("/project/malformed.json".into());
    let malformed = check_snapshot_with_options(&snapshot, &roots, &configured);
    assert!(
        has_code(&malformed, 1005),
        "missing closing brace should report TS1005: {}",
        format_diagnostics(&malformed.diagnostics)
    );
}

#[test]
fn unchecked_indexed_access_option_controls_array_index_type() {
    let path = "/project/main.ts";
    let source = "const values: number[] = []; const value: number = values[0];";
    let checked = check_sources(&[(path, source)]);
    assert_complete(&checked);
    assert!(
        has_code(&checked, 2322),
        "unchecked array access should include undefined: {}",
        format_diagnostics(&checked.diagnostics)
    );

    let mut unchecked_options = options();
    unchecked_options.no_unchecked_indexed_access = false;
    let unchecked = check_sources_with_options(&[(path, source)], &unchecked_options);
    assert_complete(&unchecked);
    assert_clean(&unchecked);
}

#[test]
fn unresolved_relative_import_reports_ts2307() {
    let path = "/project/main.ts";
    let source = "import { value } from \"./missing\"; const result = value;";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert!(
        has_code(&result, 2307),
        "unresolved import should report TS2307: {}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn package_exports_types_condition_resolves_in_snapshot() {
    let path = "/project/main.ts";
    let source = "import { value } from \"@sample/values\"; const exact: 42 = value;";
    let (mut snapshot, roots) = snapshot_with(&[(path, source)]);
    snapshot.auxiliary_files.insert(
        "/project/node_modules/@sample/values/package.json".into(),
        Arc::from(
            br#"{"name":"@sample/values","exports":{".":{"types":"./types/index.d.ts","default":"./dist/index.js"}}}"#
                .as_slice(),
        ),
    );
    let declaration = "/project/node_modules/@sample/values/types/index.d.ts";
    snapshot.modules.insert(
        declaration.into(),
        Arc::new(kiln_frontend_oxc::parse(
            declaration,
            "export declare const value: 42;",
        )),
    );
    let result = check_snapshot_with_options(&snapshot, &roots, &options());
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn import_type_resolution_mode_selects_package_condition_and_reports_invalid_mode() {
    let path = "/project/main.ts";
    let source = r#"import type { Entry as ImportEntry } from "modepkg";
type RequireEntry = import("modepkg", { with: { "resolution-mode": "require" } }).Entry;
type InvalidEntry = import("modepkg", { with: { "resolution-mode": "other" } }).Entry;
const imported: ImportEntry = "import";
const required: RequireEntry = "require";
const wrong: RequireEntry = "import";"#;
    let (mut snapshot, roots) = snapshot_with(&[(path, source)]);
    snapshot.auxiliary_files.insert(
        "/project/node_modules/modepkg/package.json".into(),
        Arc::from(
            br#"{"name":"modepkg","type":"module","exports":{".":{"import":{"types":"./types/import.d.ts","default":"./dist/esm.js"},"require":{"types":"./types/require.d.cts","default":"./dist/cjs.cjs"}}}}"#
                .as_slice(),
        ),
    );
    let import_declaration = "/project/node_modules/modepkg/types/import.d.ts";
    snapshot.modules.insert(
        import_declaration.into(),
        Arc::new(kiln_frontend_oxc::parse(
            import_declaration,
            "export type Entry = \"import\";",
        )),
    );
    let require_declaration = "/project/node_modules/modepkg/types/require.d.cts";
    snapshot.modules.insert(
        require_declaration.into(),
        Arc::new(kiln_frontend_oxc::parse(
            require_declaration,
            "export type Entry = \"require\";",
        )),
    );
    let result = check_snapshot_with_options(&snapshot, &roots, &options());
    assert_complete(&result);
    assert!(
        has_code(&result, 1453),
        "invalid resolution-mode should report TS1453: {}",
        format_diagnostics(&result.diagnostics)
    );
    assert!(
        has_code(&result, 2322),
        "require-mode import should resolve the require condition: {}",
        format_diagnostics(&result.diagnostics)
    );
    assert!(
        !has_code(&result, 2307),
        "valid import modes should resolve package exports: {}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn import_aliases_and_reexport_chains_preserve_type_and_value_bindings() {
    let (barrel, middle, main) = (
        "/project/barrel.ts",
        "/project/middle.ts",
        "/project/main.ts",
    );
    let result = check_sources(&[
        (
            barrel,
            "export interface Item { value: number } export const item: Item = { value: 1 };",
        ),
        (
            middle,
            "export { item as publicItem } from \"./barrel\"; export type { Item as PublicItem } from \"./barrel\";",
        ),
        (
            main,
            "import { publicItem as selected } from \"./middle\"; import type { PublicItem } from \"./middle\"; const checked: PublicItem = selected;",
        ),
    ]);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn malformed_typescript_preserves_parser_diagnostics_and_is_incomplete() {
    let path = "/project/main.ts";
    let source = "function add(value: number) { return value + 1; } const bad: number = ;";
    let module = kiln_frontend_oxc::parse(path, source);
    assert!(
        !module.diagnostics.is_empty(),
        "fixture should produce an owned parser diagnostic"
    );
    let result = check_sources(&[(path, source)]);
    assert!(!result.complete, "recovered malformed source is incomplete");
    for parser_diagnostic in &module.diagnostics {
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.path.as_deref() == Some(path)
                    && diagnostic.span == parser_diagnostic.span
                    && diagnostic.code.is_none()
                    && diagnostic.message == parser_diagnostic.message
            }),
            "original parser diagnostic was not preserved: {:?}; result: {:?}",
            parser_diagnostic,
            result.diagnostics
        );
    }

    let valid_module =
        kiln_frontend_oxc::parse(path, "function add(value: number) { return value + 1; }");
    let mut program = kiln_ir::compile_module(&valid_module).expect("valid source compiles");
    kiln_ir::apply_source_types(
        path,
        &valid_module,
        &result,
        crate::CHECKER_VERSION,
        &mut program.program,
    );
    let add = program
        .program
        .functions
        .iter()
        .find(|function| function.name.as_deref() == Some("add"))
        .expect("add function prototype");
    assert!(
        !add.type_annotations
            .as_ref()
            .is_some_and(|annotations| { annotations.source_types.is_some() })
    );
}

#[test]
fn resolves_numeric_parameter_alias() {
    let path = "/project/main.ts";
    let source = "type Count = number; function total(value: Count): Count { return value; }";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    let parameters = types_for(&result, path, &TypeRole::Parameter);
    assert!(
        parameters
            .iter()
            .any(|ty| ty.class == TypeClass::Number && ty.display == "number"),
        "numeric primitive alias did not resolve to number: {parameters:?}"
    );
}

#[test]
fn infers_generic_call_type() {
    let path = "/project/main.ts";
    let source = "function identity<T>(value: T): T { return value; } const answer = identity(42);";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    assert!(
        !has_code(&result, 2322),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
    assert_eq!(
        expression_type(&result, path, source, "identity(42)").class,
        TypeClass::Number
    );
}

#[test]
fn narrows_union_through_typeof_control_flow() {
    let path = "/project/main.ts";
    let source = "function label(value: string | number): string { if (typeof value === \"string\") return value.toUpperCase(); return value.toFixed(0); }";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    assert!(
        !has_code(&result, 2339),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
    assert!(
        !has_code(&result, 2322),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn checks_mapped_conditional_and_infer_types() {
    let path = "/project/main.ts";
    let source = "type Element<T> = T extends readonly (infer U)[] ? U : never; type Copy<T> = { [K in keyof T]: T[K] }; type Item = Copy<Element<readonly [{ score: number }]>>; const item: Item = { score: 3 };";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    assert!(
        !has_code(&result, 2322),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn infers_a_generic_class_argument_from_constructor_input() {
    let path = "/project/main.ts";
    let source = "class Box<T> { value: T; constructor(value: T) { this.value = value; } } const box = new Box(42); const answer: number = box.value;";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn rejects_writes_to_readonly_members() {
    let path = "/project/main.ts";
    let source = "interface Person { readonly name: string } const person: Person = { name: \"Ada\" }; person.name = \"Grace\";";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert!(
        has_code(&result, 2540),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn reports_missing_interface_members_on_implements() {
    let path = "/project/main.ts";
    let source = "interface Named { name: string } class Missing implements Named {}";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert!(
        has_code(&result, 2420),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn resolves_qualified_namespace_aliases_in_their_lexical_scope() {
    let path = "/project/main.ts";
    let source = "namespace Catalog { export type Code = string; export const code: Code = \"ok\"; } const selected: Catalog.Code = Catalog.code;";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn as_and_satisfies_keep_their_binary_expression_operand_spans() {
    let path = "/project/main.ts";
    let source = "const result = (1 as number) + (2 satisfies number);";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    assert_eq!(
        expression_type(
            &result,
            path,
            source,
            "(1 as number) + (2 satisfies number)"
        )
        .class,
        TypeClass::Number
    );
}

#[test]
fn standard_library_constructor_and_static_values_resolve() {
    let path = "/project/main.ts";
    let source = "const values = new Map<string, number>(); const predicate = Array.isArray([]);";
    let mut options = options();
    options.libraries = vec!["esnext".into()];
    let result = check_sources_with_options(&[(path, source)], &options);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn generator_next_result_types_resolve_with_esnext_libraries() {
    let path = "/project/main.ts";
    let source = "function* values() { yield 1; return \"done\"; } const made = values().next(); async function* asyncValues() { yield \"value\"; return 1; } const asyncMade = asyncValues().next();";
    let mut options = options();
    options.libraries = vec!["esnext".into()];
    let result = check_sources_with_options(&[(path, source)], &options);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn explicit_this_parameter_is_checked_by_bind_without_becoming_an_argument() {
    let path = "/project/main.ts";
    let source = "function label(this: { name: string }): string { return this.name; } label.bind({ name: \"Ada\" }); label.bind({ name: 42 });";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert!(
        has_code(&result, 2769),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
    assert!(
        !has_code(&result, 2554),
        "an explicit this parameter must not count as a positional argument: {}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn computed_symbol_iterator_as_const_and_non_null_assertion_resolve() {
    let path = "/project/main.ts";
    let source = "const literal = { kind: \"ready\" } as const; const exact: \"ready\" = literal.kind; const key = Symbol.iterator; const iterable = { [key]: 1 } as const; const possiblyMissing: 1 | undefined = iterable[key]; const direct = { [Symbol.iterator]: 1 } as const; const directValue: 1 = direct[Symbol.iterator]; const tuple = [1, \"x\"] as const; const first: 1 = tuple[0]; const maybe: string | undefined = \"value\"; const present: string = maybe!;";
    let mut options = options();
    options.libraries = vec!["esnext".into()];
    let result = check_sources_with_options(&[(path, source)], &options);
    assert_complete(&result);
    assert_clean(&result);
}

#[test]
fn infers_function_and_contextual_arrow_returns() {
    let path = "/project/main.ts";
    let source = "function explicit(): number { return 1; } const render: (value: number) => string = value => value.toFixed(1);";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    let returns = types_for(&result, path, &TypeRole::Return);
    assert!(
        returns.iter().any(|ty| ty.class == TypeClass::Number),
        "{returns:?}"
    );
    assert!(
        returns.iter().any(|ty| ty.class == TypeClass::String),
        "{returns:?}"
    );
}

#[test]
fn resolves_imported_type_alias() {
    let decl = "/project/types.ts";
    let main = "/project/main.ts";
    let result = check_sources(&[
        (
            decl,
            "export type Amount = number; export const amount = 2;",
        ),
        (
            main,
            "import { amount, type Amount } from \"./types\"; const total: Amount = amount;",
        ),
    ]);
    assert_complete(&result);
    assert_clean(&result);
    assert!(
        !has_code(&result, 2307),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
    assert!(
        !has_code(&result, 2322),
        "{}",
        format_diagnostics(&result.diagnostics)
    );
}

#[test]
fn relative_paths_resolve_imports_and_preserve_source_fact_paths() {
    let result = check_sources(&[
        ("types.ts", "export type Amount = number;"),
        (
            "main.ts",
            "import type { Amount } from \"./types\"; export function total(value: Amount): Amount { return value; }",
        ),
    ]);
    assert_complete(&result);
    assert_clean(&result);
    assert!(
        result.types.iter().any(|ty| {
            ty.path == "main.ts" && ty.role == TypeRole::Parameter && ty.class == TypeClass::Number
        }),
        "relative-path source facts did not retain the caller's path: {:?}",
        result.types
    );
}

#[test]
fn parameter_facts_classify_boolean_array_and_tuple() {
    let result = check_sources(&[(
        "main.ts",
        "export function f(a: boolean, b: number[], c: ReadonlyArray<string>, d: [number, number], e: true, g: boolean | undefined, h: {x: number}) { return 0; }",
    )]);
    assert_complete(&result);
    let classes: Vec<(String, TypeClass)> = result
        .types
        .iter()
        .filter(|ty| ty.path == "main.ts" && ty.role == TypeRole::Parameter)
        .map(|ty| (ty.display.clone(), ty.class.clone()))
        .collect();
    let class_of = |display: &str| {
        classes
            .iter()
            .find(|(d, _)| d == display)
            .unwrap_or_else(|| panic!("no parameter shown as {display}: {classes:?}"))
            .1
            .clone()
    };
    assert_eq!(class_of("boolean"), TypeClass::Boolean);
    assert_eq!(class_of("true"), TypeClass::Boolean);
    assert_eq!(class_of("number[]"), TypeClass::Array);
    assert_eq!(class_of("readonly string[]"), TypeClass::Array);
    assert_eq!(class_of("[number, number]"), TypeClass::Tuple);
    assert_eq!(class_of("{ x: number; }"), TypeClass::Composite);
    assert_eq!(class_of("boolean | undefined"), TypeClass::Composite);
}

#[test]
fn duplicate_canonical_module_paths_are_incomplete() {
    let result = check_sources(&[
        ("main.ts", "const first: number = 1;"),
        ("/project/main.ts", "const second: number = 2;"),
    ]);
    assert!(
        !result.complete && !result.diagnostics.is_empty(),
        "duplicate canonical paths must be refused with an incomplete diagnostic: {:?}",
        result
    );
}

#[test]
fn diagnostic_ranges_are_utf8_byte_offsets_after_astral_text() {
    let path = "/project/main.ts";
    let source = "const face = \"🧠\"; const bad: number = \"wrong\";";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    let diagnostic = result
        .diagnostics
        .iter()
        .find(|d| d.code == Some(2322))
        .expect("TS2322 diagnostic expected");
    let span = diagnostic.span.expect("diagnostic range expected");
    let expected_start = source.find("bad:").unwrap();
    assert_eq!(span.start as usize, expected_start);
    assert_eq!(
        &source.as_bytes()[span.start as usize..span.end as usize],
        b"bad"
    );
    assert!(expected_start > source[..expected_start].chars().count());
}

#[test]
fn any_type_facts_remain_distinct_from_number_facts() {
    let path = "/project/main.ts";
    let source =
        "function passthrough(dynamic: any) { return dynamic; } const measured: number = 1;";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    assert_eq!(
        types_for(&result, path, &TypeRole::Parameter)
            .iter()
            .next()
            .expect("dynamic parameter fact")
            .class,
        TypeClass::Any
    );
    assert_eq!(
        expression_type(&result, path, source, "1").class,
        TypeClass::Number
    );
}

#[test]
fn old_snapshot_stays_stable_after_alias_edit() {
    let entry = "/project/main.ts";
    let alias = "/project/types.ts";
    let (mut original, roots) = snapshot_with(&[
        (
            entry,
            "import type { Amount } from \"./types\"; const amount: Amount = 2;",
        ),
        (alias, "export type Amount = number;"),
    ]);
    let retained = original.clone();
    let before_edit = check(&retained, &roots, &options());
    assert_complete(&before_edit);
    assert_clean(&before_edit);
    assert!(
        !has_code(&before_edit, 2322),
        "{}",
        format_diagnostics(&before_edit.diagnostics)
    );

    original.modules.insert(
        alias.into(),
        Arc::new(kiln_frontend_oxc::parse(
            alias,
            "export type Amount = string;",
        )),
    );
    let after_edit = check(&original, &roots, &options());
    assert_complete(&after_edit);
    assert!(
        has_code(&after_edit, 2322),
        "{}",
        format_diagnostics(&after_edit.diagnostics)
    );
    assert!(
        !has_code(&before_edit, 2322),
        "retained snapshot changed after alias edit"
    );
}

#[test]
fn source_contracts_annotate_bytecode_without_changing_executable_code() {
    let path = "/project/numeric.ts";
    let source = "type Scalar = number; function add(x: Scalar) { return x + 1; }";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    let module = kiln_frontend_oxc::parse(path, source);
    let mut compiled = kiln_ir::compile_module(&module).expect("source compiles as JavaScript");
    let original_code = compiled.program.code.clone();
    let original_constants = compiled.program.constants.clone();

    kiln_ir::apply_source_types(
        path,
        &module,
        &result,
        crate::CHECKER_VERSION,
        &mut compiled.program,
    );

    let add = compiled
        .program
        .functions
        .iter()
        .find(|function| function.name.as_deref() == Some("add"))
        .expect("add function prototype");
    assert_eq!(add.parameter_type(0), Some(kiln_ir::TypeAnnotation::Number));
    assert_eq!(compiled.program.code, original_code);
    assert_eq!(compiled.program.constants, original_constants);
}

#[test]
fn transformed_method_contracts_match_their_own_signature() {
    let path = "/project/methods.ts";
    let source = r#"
type Count = number;
export class Counter {
  step(value: Count): Count { return value + 1; }
  get size(): Count { return 1; }
  ["label"](value: string): string { return value; }
}
export const object = { step(value: string): string { return value; } };
"#;
    let checked = check_sources(&[(path, source)]);
    assert_complete(&checked);
    assert_clean(&checked);
    let module = kiln_frontend_oxc::parse(path, source);
    let mut compiled = kiln_ir::compile_module(&module).unwrap();
    let executable = compiled.program.clone();
    kiln_ir::apply_source_types(
        path,
        &module,
        &checked,
        crate::CHECKER_VERSION,
        &mut compiled.program,
    );
    for (prefix, parameter, returned) in [
        (
            "step(value) {",
            Some(kiln_hir::TypeAnnotation::Number),
            kiln_hir::TypeAnnotation::Number,
        ),
        ("get size() {", None, kiln_hir::TypeAnnotation::Number),
        (
            "[\"label\"](value) {",
            Some(kiln_hir::TypeAnnotation::String),
            kiln_hir::TypeAnnotation::String,
        ),
    ] {
        let function = compiled
            .program
            .functions
            .iter()
            .find(|function| {
                compiled.program.source[function.span.start as usize..function.span.end as usize]
                    .starts_with(prefix)
            })
            .expect(prefix);
        assert_eq!(function.parameter_type(0), parameter);
        assert_eq!(function.return_type(), Some(returned));
    }
    let string_step = compiled
        .program
        .functions
        .iter()
        .filter(|function| {
            compiled.program.source[function.span.start as usize..function.span.end as usize]
                .starts_with("step(value) {")
        })
        .last()
        .unwrap();
    assert_eq!(
        string_step.parameter_type(0),
        Some(kiln_hir::TypeAnnotation::String)
    );
    for (before, after) in executable.functions.iter().zip(&compiled.program.functions) {
        assert_eq!(before.code, after.code);
        if after.is_class {
            assert!(
                after
                    .type_annotations
                    .as_ref()
                    .is_none_or(|annotations| annotations.source_types.is_none())
            );
        }
    }
}

#[test]
fn function_contract_fact_selection_preserves_checking_and_bytecode_metadata() {
    let path = "/project/contracts.ts";
    let source = "type Scalar = number; function add(value: Scalar): Scalar { return value + 1; } const answer = add(41);";
    let mut all_options = options();
    all_options.libraries = vec!["esnext".into()];
    let all = check_sources_with_options(&[(path, source)], &all_options);
    assert_complete(&all);

    let mut contract_options = all_options.clone();
    contract_options.facts = FactSelection::FunctionContracts;
    let contracts = check_sources_with_options(&[(path, source)], &contract_options);
    assert_eq!(contracts.complete, all.complete);
    assert_eq!(contracts.diagnostics, all.diagnostics);
    assert!(
        all.types
            .iter()
            .any(|fact| fact.role == TypeRole::Expression),
        "full fact selection should include expression facts"
    );
    assert!(
        contracts
            .types
            .iter()
            .all(|fact| fact.role != TypeRole::Expression),
        "function-contract selection should omit expression facts"
    );
    let contract_facts = |result: &CheckResult| {
        result
            .types
            .iter()
            .filter(|fact| matches!(&fact.role, TypeRole::Parameter | TypeRole::Return))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(contract_facts(&all), contract_facts(&contracts));

    let module = kiln_frontend_oxc::parse(path, source);
    let mut all_program = kiln_ir::compile_module(&module).expect("source compiles as JavaScript");
    let mut contract_program =
        kiln_ir::compile_module(&module).expect("source compiles as JavaScript");
    kiln_ir::apply_source_types(
        path,
        &module,
        &all,
        crate::CHECKER_VERSION,
        &mut all_program.program,
    );
    kiln_ir::apply_source_types(
        path,
        &module,
        &contracts,
        crate::CHECKER_VERSION,
        &mut contract_program.program,
    );
    let source_annotations = |program: &kiln_ir::Program| {
        program
            .functions
            .iter()
            .find(|function| function.name.as_deref() == Some("add"))
            .and_then(|function| function.type_annotations.as_ref())
            .and_then(|annotations| annotations.source_types.clone())
            .expect("add function source contracts")
    };
    assert_eq!(
        source_annotations(&all_program.program),
        source_annotations(&contract_program.program)
    );
}

#[test]
fn any_parameter_does_not_become_a_numeric_bytecode_contract() {
    let path = "/project/dynamic.ts";
    let source = "function add(value: any) { return value + 1; }";
    let result = check_sources(&[(path, source)]);
    assert_complete(&result);
    assert_clean(&result);
    assert!(
        types_for(&result, path, &TypeRole::Parameter)
            .iter()
            .any(|ty| ty.class == TypeClass::Any)
    );
    let module = kiln_frontend_oxc::parse(path, source);
    let mut compiled = kiln_ir::compile_module(&module).expect("source compiles as JavaScript");
    let original_code = compiled.program.code.clone();
    let original_constants = compiled.program.constants.clone();

    kiln_ir::apply_source_types(
        path,
        &module,
        &result,
        crate::CHECKER_VERSION,
        &mut compiled.program,
    );

    let add = compiled
        .program
        .functions
        .iter()
        .find(|function| function.name.as_deref() == Some("add"))
        .expect("add function prototype");
    assert_ne!(add.parameter_type(0), Some(kiln_ir::TypeAnnotation::Number));
    assert_eq!(compiled.program.code, original_code);
    assert_eq!(compiled.program.constants, original_constants);
}

/// `KILN_LOWER_BENCH=<iterations> cargo test --release -p kiln-type-checker -- --ignored lower_bench`
#[test]
#[ignore]
fn lower_bench() {
    use crate::engine::{atom::Interner, session::Session};
    let source = crate::STANDARD_LIBRARIES
        .iter()
        .find(|(name, _)| *name == "lib.es5.d.ts")
        .unwrap()
        .1;
    let module = kiln_frontend_oxc::parse_authored("/l/lib.es5.d.ts", source);
    let iterations: u32 = std::env::var("KILN_LOWER_BENCH")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let session = Session::new();
    let atoms = Interner::new_in(&session);
    let started = std::time::Instant::now();
    for _ in 0..iterations {
        let (file, _) = crate::lower::Lower::new(&module, &atoms).finish(session.arena(), &session);
        std::hint::black_box(&file);
    }
    eprintln!(
        "lower lib.es5.d.ts: {:?} each",
        started.elapsed() / iterations
    );
}

/// A source with an adapter gap the checker reports: a compiler capability expression appended
/// to its authored body, as `checker_marks_unrepresentable_generated_capability_expressions_incomplete`
/// builds one.
fn with_gap(snapshot: &mut SourceSnapshot, path: &str, source: &str) {
    let mut module = kiln_frontend_oxc::parse_static_typescript(
        path,
        source,
        kiln_frontend_oxc::StaticTypescriptPolicy::PreserveAuthoredSource,
    );
    let span = kiln_hir::Span::new(0, 1);
    module.type_syntax.body.push(kiln_hir::Statement {
        kind: kiln_hir::StatementKind::Expression(kiln_hir::Expression {
            kind: kiln_hir::ExpressionKind::GlobalObject,
            span,
        }),
        span,
    });
    snapshot.modules.insert(path.into(), Arc::new(module));
}

#[test]
fn a_gap_leaves_the_files_that_do_not_reach_it_complete() {
    let (main, leaf) = ("/project/main.ts", "/project/leaf.ts");
    let main_source = "import { leaf } from './leaf';\nexport const value = leaf;";
    let (mut snapshot, roots) =
        snapshot_with(&[(main, main_source), (leaf, "export const leaf = 1;")]);
    with_gap(&mut snapshot, main, main_source);
    let result = check_snapshot_with_options(&snapshot, &roots, &options());
    assert!(!result.complete);
    assert!(result.covers(main) && result.covers(leaf));
    assert!(result.is_complete_for(leaf), "{:?}", result.files);
    assert!(!result.is_complete_for(main), "{:?}", result.files);
}

#[test]
fn a_gap_in_a_script_leaves_no_file_complete() {
    let (script, leaf) = ("/project/script.ts", "/project/leaf.ts");
    let script_source = "const scriptValue = 1;";
    let (mut snapshot, roots) =
        snapshot_with(&[(script, script_source), (leaf, "export const leaf = 1;")]);
    with_gap(&mut snapshot, script, script_source);
    let result = check_snapshot_with_options(&snapshot, &roots, &options());
    assert!(!result.complete);
    assert!(!result.is_complete_for(leaf), "{:?}", result.files);
}

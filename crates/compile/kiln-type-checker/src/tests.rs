use std::sync::{Arc, OnceLock};

use crate::{
    CheckOptions, CheckResult, SourceDiagnostic, SourceSnapshot, SourceType, TypeClass, TypeRole,
    check,
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

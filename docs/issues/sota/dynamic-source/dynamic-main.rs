use kiln_hir::SourceGoal;
use kiln_interpreter::{EvalCompileError, EvalSource, ExecutionCapabilities};
use kiln_ir::compile_module;

fn compile_dynamic(
    request: &EvalSource<'_>,
) -> Result<(kiln_ir::Program, kiln_ir::Strictness), EvalCompileError> {
    let units = request
        .source
        .encode_utf16()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");
    println!("compiler-source={units}");
    Ok((
        kiln_ir::Program {
            source: request.source.to_owned().into(),
            template_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            constants: vec![kiln_ir::Constant::Number(1.0f64.to_bits())],
            code: vec![kiln_ir::Instruction::Const(0), kiln_ir::Instruction::Return],
            local_count: request.outer.len() as u32,
            functions: Vec::new(),
        },
        kiln_ir::Strictness::default(),
    ))
}

fn run_dynamic_case(label: &str, source: &str, expect_refusal: bool) {
    let dynamic_module = compile_module(&kiln_frontend_oxc::parse_with_goal(
        "dynamic.js",
        source,
        SourceGoal::Script,
    ))
    .expect("compile dynamic source");
    let mut dynamic_host = kiln_runtime::CaptureHost::new();
    let dynamic_result = kiln_interpreter::execute_module(
        &dynamic_module,
        &[],
        &mut dynamic_host,
        Default::default(),
        ExecutionCapabilities::default().with_source_compiler(compile_dynamic),
    );
    println!("{label}-stdout={:?}", dynamic_host.stdout());
    match dynamic_result {
        Ok(result) => {
            assert!(
                !expect_refusal,
                "{label} unexpectedly reached the stub source compiler"
            );
            println!("{label}-result={:?}", result.execution.value);
        }
        Err(kiln_interpreter::ExecutionError::EvalUnsupported { message }) => {
            assert!(expect_refusal, "{label} unexpectedly refused valid source");
            assert_eq!(
                message,
                "dynamic source contains unpaired UTF-16 surrogate code units"
            );
            println!("{label}-error=EvalUnsupported {{ message: {message:?} }}");
        }
        Err(error) => panic!("{label} returned an unexpected dynamic-source error: {error:?}"),
    }
}

fn main() {
    run_dynamic_case(
        "raw-quoted",
        r#"
const high = String.fromCharCode(0xd800);
eval('"' + high + '"');
"#,
        true,
    );
    run_dynamic_case(
        "escaped-quoted",
        r#"
const high = String.fromCharCode(0xd800);
eval('"' + '\\uD800' + '"');
"#,
        false,
    );
    run_dynamic_case(
        "raw-outside-literal",
        r#"
const high = String.fromCharCode(0xd800);
eval(high);
"#,
        true,
    );

    let accessor_source =
        include_str!("../../../conformance/native/inferred-name-auto-accessor.ts");
    let accessor_module = compile_module(&kiln_frontend_oxc::parse_with_goal(
        "inferred-name-auto-accessor.ts",
        accessor_source,
        SourceGoal::Script,
    ))
    .expect("compile auto-accessor source");
    let mut accessor_host = kiln_runtime::CaptureHost::new();
    let accessor_result = kiln_interpreter::execute_module(
        &accessor_module,
        &[],
        &mut accessor_host,
        Default::default(),
        ExecutionCapabilities::default(),
    )
    .expect("execute auto-accessor source");
    assert_eq!(
        accessor_host.stdout(),
        "11 22\n101 202\n",
        "auto-accessor public keys lost their exact UTF-16 identity"
    );
    println!("auto-accessor-stdout={:?}", accessor_host.stdout());
    println!("auto-accessor-result={:?}", accessor_result.execution.value);
}

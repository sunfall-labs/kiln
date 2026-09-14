use kiln_interpreter::{EvalCompileError, EvalSource, ExecutionCapabilities};
use kiln_ir::{compile_eval_with_strictness, compile_module, EvalOptions};
use kiln_hir::SourceGoal;

fn compile_dynamic(request: &EvalSource<'_>) -> Result<(kiln_ir::Program, kiln_ir::Strictness), EvalCompileError> {
    let units = request.source.encode_utf16().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
    println!("compiler-source={units}");
    Ok((kiln_ir::Program {
        source: request.source.to_owned().into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![kiln_ir::Constant::Number(1.0f64.to_bits())],
        code: vec![kiln_ir::Instruction::Const(0), kiln_ir::Instruction::Return],
        local_count: request.outer.len() as u32,
        functions: Vec::new(),
    }, kiln_ir::Strictness::default()))
}

fn main() {
    let source = r#"
const high = String.fromCharCode(0xd800);
eval('"' + high + '"');
eval('"' + '\\uD800' + '"');
try { eval(high); } catch (error) { console.log(error.name); }
"#;
    let module = compile_module(&frontend_oxc::parse_with_goal("dynamic.js", source, SourceGoal::Script))
        .expect("compile source");
    let mut host = kiln_runtime::CaptureHost::new();
    let result = kiln_interpreter::execute_module(
        &module,
        &[],
        &mut host,
        Default::default(),
        ExecutionCapabilities::default().with_source_compiler(compile_dynamic),
    ).expect("execute source");
    println!("stdout={:?}", host.stdout());
    println!("result={:?}", result.execution.value);
}

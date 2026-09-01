//! The frontend `eval` and `Function` reach for while a program is already running.
//!
//! The interpreter deliberately owns no parser (ADR 0007), so it asks for one instead: a host
//! that wants `eval` installs a function that turns source text into a program, exactly as
//! `kiln-db` takes its frontend as a function pointer (ADR 0006). This crate is such a host, and
//! this is the same `frontend_oxc` → `kiln_ir` path every other run in the suite takes rather
//! than a second, conformance-only compiler.

use frontend_oxc::parse_with_goal;
use kiln_hir::SourceGoal;
use kiln_interpreter::{EvalCompileError, EvalSource, ExecutionCapabilities};
use kiln_ir::{EvalOptions, Program, Strictness};

/// Give this thread's VMs a compiler, so `eval` and `Function` can build a program at run time.
///
/// Idempotent, and called at the top of every run rather than once at startup: a conformance run
/// creates a VM per case and this crate has no other moment that happens exactly once.
pub(crate) fn capabilities() -> ExecutionCapabilities {
    ExecutionCapabilities::default()
        .with_source_compiler(compile_source)
        .with_module_loader(crate::module_loader::load)
        // Test262's `$262.createRealm`. A host capability rather than a language feature, so it
        // is named here beside the compiler rather than assumed by the engine.
        .with_host_realms()
}

fn compile_source(request: &EvalSource<'_>) -> Result<(Program, Strictness), EvalCompileError> {
    // A strict eval is prepended with the directive rather than told about it, because the
    // *parser* is where strict-mode early errors live -- `delete x`, `with`, a duplicate
    // parameter -- and it takes its answer from the source it is given. The directive itself
    // does not survive lowering, so it cannot become the eval's completion value.
    let source = if request.strict {
        format!("\"use strict\";\n{}", request.source)
    } else {
        request.source.to_owned()
    };
    // Always a Script. Eval code cannot import or export, and choosing the module goal would
    // silently accept syntax the language does not allow here.
    let module = parse_with_goal("<eval>", &source, SourceGoal::Script);
    if let Some(diagnostic) = module.diagnostics.first() {
        return Err(EvalCompileError::Syntax(diagnostic.message.clone()));
    }
    if module.panicked {
        return Err(EvalCompileError::Syntax(
            "source could not be parsed as a program".to_owned(),
        ));
    }
    kiln_ir::compile_eval_with_strictness(
        &module,
        &EvalOptions {
            outer: request.outer.to_vec(),
            strict: request.strict,
            global_var_scope: request.global_var_scope,
        },
    )
    .map_err(|diagnostics| {
        let message = diagnostics.first().map_or_else(
            || "source could not be compiled".to_owned(),
            |d| d.message.clone(),
        );
        // A gap in this compiler is not the language rejecting the source. Reporting
        // `SyntaxError` for one would tell the program its own text was invalid, and a program
        // that believes that takes the branch it wrote for bad input instead of failing.
        if diagnostics
            .iter()
            .all(|diagnostic| crate::test262::is_intentionally_unsupported(&diagnostic.message))
        {
            EvalCompileError::Unsupported(message)
        } else {
            EvalCompileError::Syntax(message)
        }
    })
}

use super::*;
use kiln_hir::{
    AssignmentOperator, BinaryOperator, CatchClause, Class, ClassField, ClassInitialiser,
    ExportName, ExportOrigin, Expression, ExpressionKind, Function, ImportName, ListElement,
    Literal, LogicalOperator, Module, ModuleRequestKind, ObjectKey, ObjectMember, Pattern,
    PatternKey, SourceGoal, Statement, StatementKind, SwitchCase, UnaryOperator, VariableKind,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Compile owned HIR into stack bytecode.
pub fn compile(module: &Module) -> Result<Program, Vec<CompileDiagnostic>> {
    compile_with_options(module, CompileOptions::default())
}

/// Compile owned HIR into stack bytecode, saying what the source text alone cannot.
pub fn compile_with_options(
    module: &Module,
    options: CompileOptions,
) -> Result<Program, Vec<CompileDiagnostic>> {
    Ok(compile_with_strictness(module, options)?.0)
}

/// [`compile_with_options`], also reporting which bodies came out strict.
///
/// A second entry point rather than a wider return type, because only the interpreter needs the
/// answer and every other caller of `compile` would have to thread a value it never reads.
pub fn compile_with_strictness(
    module: &Module,
    options: CompileOptions,
) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    // Either the caller knows -- a Test262 strict variant, a `Function` body -- or the source
    // opened with the directive itself, or the goal settles it: a Module is strict whatever its
    // text says. Without the goal this entry point was the one place a module body could be
    // compiled sloppy, which now decides more than a refused write -- it decides whether an
    // assignment to an unbound name creates a global.
    compiler.strict =
        options.strict || module.strict || matches!(module.source_goal, SourceGoal::Module);
    // A script's `CreateGlobalVarBinding` is called with `D` false, so `var x` at the top of a
    // script cannot be deleted afterwards. Strict global code still puts its `var`s on the
    // object -- strictness decides what an *unresolvable* assignment does, not where a
    // declaration lives.
    compiler.global_var_scope =
        options.global_code && !matches!(module.source_goal, SourceGoal::Module);
    compiler.global_bindings_configurable = false;
    // The module body has nested functions too, so its own block bindings can be captured.

    for requested in module.runtime_requested_modules() {
        if let Some(occurrence) = requested.occurrences.first() {
            compiler.error(
                Some(occurrence.specifier_span),
                format!(
                    "module dependency `{}` is not supported by this execution backend",
                    requested.specifier
                ),
            );
        }
    }
    for export in &module.exports {
        if export.type_only {
            continue;
        }
        match &export.origin {
            ExportOrigin::ReExport { source, .. }
            | ExportOrigin::Star { source }
            | ExportOrigin::NamespaceReExport { source } => compiler.error(
                Some(export.span),
                format!("re-export from `{source}` is not supported by this execution backend"),
            ),
            ExportOrigin::Unsupported { description, .. } => compiler.error(
                Some(export.span),
                format!("unsupported export: {description}"),
            ),
            ExportOrigin::Local { .. } | ExportOrigin::DefaultExpression => {}
        }
    }

    compiler.compile_body(module);
    compiler.finish_with_strictness()
}

/// One binding an eval program borrows from the frame that called it.
///
/// The cell itself travels at run time; only its name and whether it may be written are the
/// compiler's business. Order matters: the *i*th entry binds slot *i* of the produced program,
/// which is the whole contract between [`compile_eval`] and the machine that installs the cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOuter {
    pub name: String,
    pub declaration: EvalDeclaration,
}

/// What a run-time `eval` compilation needs beyond the source it was handed.
#[derive(Debug, Clone, Default)]
pub struct EvalOptions {
    /// The calling frame's visible bindings, innermost first. Empty for an indirect eval, which
    /// sees global scope and nothing else.
    pub outer: Vec<EvalOuter>,
    /// Names among `outer` whose cells hold active `with` object environments, outermost first.
    pub with_objects: Vec<String>,
    /// Strict either because the caller is, or because the eval'd source says so itself.
    pub strict: bool,
    /// Whether the variable environment this eval's declarations belong to is the global one.
    ///
    /// Always true for an indirect eval, which *is* global code. True for a direct one only when
    /// the call is written at the top level of a script.
    pub global_var_scope: bool,
    /// Whether this source is a *Script* rather than eval code.
    ///
    /// Two things separate the two, and both are visible to a program. A script's `var`s go on
    /// the global object even in strict code -- strictness decides what an unresolvable
    /// assignment does, not where a declaration lives -- while a strict eval gets a variable
    /// environment of its own. And `CreateGlobalVarBinding` is called with `D` false for a
    /// script and true for an eval, so one written in a script cannot afterwards be deleted and
    /// one written in an `eval` can.
    ///
    /// This is what Test262's `$262.evalScript` asks for, and the reason it is not spelled
    /// `eval`: a host that answered it with `eval` would report every binding it made as
    /// configurable, which is the one thing those tests are checking.
    pub script: bool,
    /// The name of the caller's home-object binding, when `super.x` is legal at the call site.
    /// The binding arrives among `outer`; this says which of those names is the home object.
    pub home: Option<String>,
    /// The private names the call site can see, outermost class first.
    pub private_scopes: Vec<PrivateScope>,
    /// Names a `var` in this source may not take over, from [`EvalSite::blocked_var_names`].
    pub blocked_var_names: Vec<String>,
}

///
/// Two things separate this from [`compile`]. The program *completes with a value* -- `eval("1")`
/// is 1 -- where a module body always completes with `undefined`. And its scope 0 may already
/// hold bindings: a direct eval reads and writes the caller's, through cells the machine installs
/// in slots 0..`outer.len()` before the first instruction runs.
pub fn compile_eval(
    module: &Module,
    options: &EvalOptions,
) -> Result<Program, Vec<CompileDiagnostic>> {
    Ok(compile_eval_with_strictness(module, options)?.0)
}

/// [`compile_eval`], also reporting which bodies came out strict.
pub fn compile_eval_with_strictness(
    module: &Module,
    options: &EvalOptions,
) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    compiler.strict = options.strict || module.strict;
    compiler.eval_program = true;
    // `EvalDeclarationInstantiation` puts an eval's `var`s in the calling context's variable
    // environment, and when that *is* the global environment record they become properties of
    // the global object -- created with `D` true, which is why one written inside an `eval` can
    // be deleted and one written in a script cannot. Strict eval code gets a variable
    // environment of its own and never reaches here.
    compiler.global_var_scope = options.global_var_scope && (options.script || !compiler.strict);
    compiler.global_bindings_configurable = !options.script;
    // Unconditional, unlike an ordinary program, and it is what makes the eval'd code reachable
    // from itself. A module-scope binding that is *not* boxed is read from a nested function
    // through `LoadGlobal`, which finds the frame of whichever module is running -- and for an
    // eval that is the caller's module, holding entirely unrelated slots. Boxing every top-level
    // binding means every such read is a capture instead, and `LoadGlobal` is never emitted.
    compiler.cell_everything = true;
    for statement in &module.body {
        match &statement.kind {
            StatementKind::Variable { kind, declarations } if *kind != VariableKind::Var => {
                for declaration in declarations {
                    let mut names = Vec::new();
                    analysis::pattern_names_in_order(&declaration.pattern, &mut names);
                    compiler.eval_lexical_names.extend(names);
                }
            }
            StatementKind::Class(class) => {
                if let Some(name) = &class.name {
                    compiler.eval_lexical_names.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    // Strict eval code gets a variable environment of its own, so nothing it declares can
    // collide with an environment it is nested in and the whole rule is silent there.
    if !compiler.strict {
        compiler.blocked_var_names = options.blocked_var_names.clone();
    }
    for outer in &options.outer {
        let slot = compiler.local_count;
        compiler.local_count += 1;
        compiler.outer_names.insert(outer.name.clone());
        compiler.scopes[0].insert(
            outer.name.clone(),
            Local {
                slot,
                kind: match outer.declaration {
                    EvalDeclaration::Var => VariableKind::Var,
                    EvalDeclaration::Let => VariableKind::Let,
                    // Keep a borrowed immutable binding syntactically assignable in the eval
                    // compiler so the captured-cell guard can produce the runtime TypeError.
                    // Treating it as a source `const` would reject an eval-created function at
                    // compile time, before the write can run. Its original declaration remains
                    // in `eval_declaration` for nested eval sites and backend metadata.
                    EvalDeclaration::Const => VariableKind::Let,
                    EvalDeclaration::FunctionName => VariableKind::Let,
                },
                imported: false,
                initialized: true,
                // The caller's cell is installed in the slot before the body runs, so there is
                // nothing for a prologue to box and every access goes straight through it.
                celled: true,
                eval_declaration: Some(outer.declaration),
            },
        );
    }
    for name in &options.with_objects {
        compiler.with_scopes.push(WithScope {
            name: name.clone(),
            functions: 0,
            // Eval source declarations occupy scope zero too; the compiler separately marks
            // its own lexical names so they shadow these enclosing object environments.
            scopes: 1,
        });
    }
    // The enclosing classes' private names, so `eval("this.#m")` resolves `#m` to the element
    // the class actually declared rather than to a key of its own. The identities come with them
    // for the reason `PrivateScope` records, and the counter is advanced past every one: a class
    // declared *inside* this eval must not be handed an identity the caller is already using, or
    // its brand check would accept the caller's instances.
    for scope in &options.private_scopes {
        compiler.private_scopes.push(scope.scope);
        compiler.private_scope_modules.push(scope.module_id.clone());
        compiler
            .private_declarations
            .push(scope.names.iter().cloned().collect());
        compiler.next_private_scope = compiler.next_private_scope.max(scope.scope + 1);
    }
    // `super.x` in the eval'd source reads through the caller's home binding, which arrived as
    // one of the outer names above and so is already a slot of scope 0.
    if let Some(home) = &options.home {
        compiler.home_bindings.push(home.clone());
    }
    compiler.compile_eval_body(module);
    compiler.finish_with_strictness()
}

/// Compile one module while preserving the scalar linkage needed by a graph linker.
///
/// Named and default value imports are supported. Namespace imports, star/namespace re-exports,
/// indirect re-exports and default expression exports remain explicit unsupported forms.
pub fn compile_module(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    let mut compiler = Compiler::new(module);
    // A module is strict whatever its text says, so nothing has to pass it in.
    compiler.strict = true;
    compiler.module_body = true;
    let mut imports = Vec::new();

    for import in module.imports.iter().filter(|import| !import.type_only) {
        let imported = match &import.imported {
            ImportName::Name(name) => name.clone(),
            ImportName::Default => "default".to_owned(),
            // `import * as ns` asks for every export at once. The name carries characters an
            // identifier cannot, so it can never collide with a real export.
            ImportName::Namespace => NAMESPACE_IMPORT.to_owned(),
        };
        if let Some(local_slot) = compiler.declare_import(&import.local, import.span) {
            imports.push(ImportBinding {
                source: import.source.clone(),
                imported,
                local: import.local.clone(),
                local_slot,
            });
        }
    }

    // `export { x } from "./m"` is an import the module never names plus an export of what it
    // brought in. Desugaring it here rather than teaching the linker a third kind of export
    // means the import path and the local-export path each stay the only one of their kind.
    //
    // The value is read once, when the provider finishes, which is where every other export is
    // read too. A genuinely live re-export -- one that shows a later reassignment in the
    // provider -- needs exported cells rather than exported values, and is a separate change.
    let mut reexport_slots: HashMap<usize, u32> = HashMap::new();
    let mut star_exports: Vec<ExportBinding> = Vec::new();
    for (index, export) in module.exports.iter().enumerate() {
        if export.type_only {
            continue;
        }
        let (source, imported) = match &export.origin {
            ExportOrigin::ReExport { source, imported } => (
                source,
                match imported {
                    ImportName::Name(name) => name.clone(),
                    ImportName::Default => "default".to_owned(),
                    ImportName::Namespace => NAMESPACE_IMPORT.to_owned(),
                },
            ),
            ExportOrigin::NamespaceReExport { source } | ExportOrigin::Star { source } => {
                (source, NAMESPACE_IMPORT.to_owned())
            }
            _ => continue,
        };
        // A name no program can write, and one per re-export, so two of them from the same
        // module do not share a slot.
        let local = format!("{REEXPORT_LOCAL_PREFIX}{index}*");
        let Some(local_slot) = compiler.declare_import(&local, export.span) else {
            continue;
        };
        // `export *` names nothing, so the main loop below -- which needs an exported name --
        // never sees it. Its export entry is made here instead, naming the source.
        if let ExportOrigin::Star { source } = &export.origin {
            let exported = format!("{STAR_REEXPORT_PREFIX}{source}");
            if !star_exports.iter().any(|e| e.exported == exported) {
                star_exports.push(ExportBinding {
                    exported,
                    local_slot,
                });
            }
        } else {
            reexport_slots.insert(index, local_slot);
        }
        imports.push(ImportBinding {
            source: source.clone(),
            imported,
            local,
            local_slot,
        });
    }

    // Every ESM import is a live binding. A native graph may need to expose this module's
    // binding while the module is still in its instantiation phase, before its body initializes
    // a const or function declaration. Exported locals therefore use the same cell representation
    // as mutable exports, so importers retain a live view through cyclic evaluation.
    for export in &module.exports {
        match &export.origin {
            ExportOrigin::Local { local } => {
                compiler.captured.insert(local.clone());
            }
            ExportOrigin::DefaultExpression => {
                compiler
                    .captured
                    .insert(kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
            }
            _ => {}
        }
    }
    compiler.compile_body(module);

    let mut exports = star_exports;
    for (index, export) in module
        .exports
        .iter()
        .enumerate()
        .filter(|(_, export)| !export.type_only)
    {
        let Some(exported) = export.exported.as_ref().map(ExportName::as_str) else {
            if !matches!(export.origin, ExportOrigin::Star { .. }) {
                compiler.error(Some(export.span), "runtime export has no exported name");
            }
            continue;
        };
        match &export.origin {
            ExportOrigin::Local { local } => match compiler.resolve_module_local(local) {
                Some(local_slot) => exports.push(ExportBinding {
                    exported: exported.to_owned(),
                    local_slot,
                }),
                None => compiler.error(
                    Some(export.span),
                    format!("export `{exported}` refers to missing local `{local}`"),
                ),
            },
            ExportOrigin::DefaultExpression => {
                match compiler.resolve_module_local(kiln_hir::DEFAULT_EXPORT_LOCAL) {
                    Some(local_slot) => exports.push(ExportBinding {
                        exported: exported.to_owned(),
                        local_slot,
                    }),
                    None => {
                        compiler.error(Some(export.span), "`export default` has no value to export")
                    }
                }
            }
            ExportOrigin::ReExport { .. } | ExportOrigin::NamespaceReExport { .. } => {
                if let Some(local_slot) = reexport_slots.get(&index) {
                    exports.push(ExportBinding {
                        exported: exported.to_owned(),
                        local_slot: *local_slot,
                    });
                }
            }
            // Handled above: a star export has no exported name, so control never reaches here.
            ExportOrigin::Star { .. } => {}
            ExportOrigin::Unsupported { description, .. } => compiler.error(
                Some(export.span),
                format!("unsupported export: {description}"),
            ),
        }
    }

    let mut dependencies = Vec::new();
    for request in module.runtime_requested_modules() {
        if !dependencies.contains(&request.specifier) {
            dependencies.push(request.specifier.clone());
        }
    }

    let link_facts = ModuleLinkFacts {
        node_runtime_requests: module.node_runtime_requests.clone(),
        immutable_bindings: immutable_module_bindings(module),
        direct_eval: body_contains_direct_eval(&module.body),
        new_function: body_contains_new_function(&module.body),
        dynamic_imports: collect_dynamic_imports(&module.body),
        removable_declarations: compiler.removable_declarations.clone(),
        module_instantiation_end: compiler.module_instantiation_end,
    };

    compiler
        .finish_with_strictness()
        .map(|(program, strictness)| ModuleProgram {
            program,
            dependencies,
            imports,
            exports,
            strictness,
            link_facts,
        })
}

/// One parameter's share of `FunctionDeclarationInstantiation`, in declaration order.
///
/// The list is built while the parameters are being declared and emitted afterwards, because
/// the two need different information and the order between them is the same: left to right,
/// which is the order a default expression can observe.
enum ParameterStep<'a> {
    /// A pattern, destructured out of its positional slot.
    Destructure(u32, &'a Pattern, Span),
    /// An identifier with a default, assigned from its positional slot.
    Defaulted(u32, bool, &'a Pattern, Span, &'a str),
    /// A plain identifier: the call already put it in its slot, and nothing needs emitting.
    Plain(&'a str),
}

/// Static `import`/`export` syntax with runtime meaning, refused in CommonJS.
///
/// Shared by [`compile_commonjs`] and the graph providers, which must report the same
/// refusal when a CommonJS-classified file is explicit CJS (`.cjs`, `.cts`, or an explicit
/// `"type": "commonjs"` manifest) rather than syntax-detected as ESM.
pub fn cjs_static_module_syntax_diagnostics(module: &Module) -> Vec<CompileDiagnostic> {
    for requested in &module.requested_modules {
        for occurrence in &requested.occurrences {
            if occurrence.type_only {
                continue;
            }
            let message = match occurrence.kind {
                ModuleRequestKind::Import => "Cannot use import statement outside a module",
                ModuleRequestKind::Export => "Cannot use export statement outside a module",
            };
            return vec![CompileDiagnostic {
                message: message.to_owned(),
                span: Some(occurrence.statement_span),
            }];
        }
    }
    // The occurrence walk above covers every statement the frontend records. The two entry
    // lists are checked too, so hand-built HIR with entries but no occurrences cannot slip
    // through either.
    if let Some(import) = module.imports.iter().find(|import| !import.type_only) {
        return vec![CompileDiagnostic {
            message: "Cannot use import statement outside a module".to_owned(),
            span: Some(import.statement_span),
        }];
    }
    if let Some(export) = module.exports.iter().find(|export| !export.type_only) {
        return vec![CompileDiagnostic {
            message: "Cannot use export statement outside a module".to_owned(),
            span: Some(export.statement_span),
        }];
    }
    Vec::new()
}

/// Compile a CommonJS body with Node's wrapper bindings installed as module-scope cells.
///
/// The graph supplies the five values in this exact order. They are ordinary mutable bindings:
/// `module.exports = value` must be observable after evaluation, and nested functions capture
/// `require`/`exports` through the same cell mechanism as any other module-scope name.
pub fn compile_commonjs(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    const WRAPPER: [&str; 5] = ["exports", "require", "module", "__filename", "__dirname"];
    // Static module syntax is a SyntaxError in CommonJS, exactly as Node reports it. Without
    // this the bindings below are never declared and the program fails later with an unbound
    // name (`fs is not defined`) or a silent `undefined` -- the failure class this repository
    // refuses to produce. `import type` and `export type` are erased before this point and stay
    // legal; every other static form is refused at its own statement span, in source order.
    let syntax_diagnostics = cjs_static_module_syntax_diagnostics(module);
    if !syntax_diagnostics.is_empty() {
        return Err(syntax_diagnostics);
    }
    let mut compiler = Compiler::new(module);
    // Node wraps a CommonJS file in an ordinary function, so the body is *sloppy* unless its own
    // text opens with the directive. That is not a detail: it decides whether `undeclaredGlobal =
    // 1` installs a global -- which is how a UMD header attaches itself -- and whether `this` in a
    // bare call inside the module is the global object. A Module goal would be strict whatever
    // its text said; this goal is the other one, and this is where the difference is recorded.
    compiler.strict = module.strict;
    // A CommonJS module's values routinely leave their defining module through `module.exports`
    // and are invoked by importers. Module-slot instructions are caller-relative in the VM, so
    // no CommonJS function may depend on one: its defining bindings have to travel as cells.
    // Boxing the whole top level makes that invariant structural rather than dependent on a
    // syntax-shape capture scan keeping pace with every form emitted by npm packages (notably
    // destructured parameter defaults in path-to-regexp).
    compiler.cell_everything = true;
    let mut imports = Vec::with_capacity(WRAPPER.len());
    for name in WRAPPER {
        let slot = compiler.local_count;
        compiler.local_count += 1;
        compiler.outer_names.insert(name.to_owned());
        compiler.scopes[0].insert(
            name.to_owned(),
            Local {
                slot,
                // Node supplies these as function parameters. A `var` declaration of a
                // parameter is legal and reuses that binding (`var module = ...` appears in
                // real packages); modelling them as lexical `let`s rejects valid CommonJS.
                kind: VariableKind::Var,
                imported: false,
                initialized: true,
                celled: true,
                eval_declaration: None,
            },
        );
        imports.push(ImportBinding {
            source: "*commonjs-wrapper*".into(),
            imported: name.into(),
            local: name.into(),
            local_slot: slot,
        });
    }
    compiler.compile_body(module);
    let link_facts = ModuleLinkFacts {
        node_runtime_requests: module.node_runtime_requests.clone(),
        immutable_bindings: immutable_module_bindings(module),
        direct_eval: body_contains_direct_eval(&module.body),
        new_function: body_contains_new_function(&module.body),
        dynamic_imports: collect_dynamic_imports(&module.body),
        removable_declarations: Vec::new(),
        module_instantiation_end: 0,
    };
    compiler
        .finish_with_strictness()
        .map(|(program, strictness)| ModuleProgram {
            program,
            dependencies: Vec::new(),
            imports,
            exports: Vec::new(),
            strictness,
            link_facts,
        })
}

/// Unpatched exits from one loop.
///
/// Collected rather than patched as they are emitted, because neither target is known until the
/// loop's shape is finished: `break` needs the address after the loop and `continue` needs the
/// update clause.
/// Something that has to run when control leaves the region it guards.
///
/// A `finally` block and closing a `for-of` iterator are the same idea -- run this on every way
/// out -- and every non-local exit already knows how to walk one stack of them. Keeping them in
/// one stack is what makes `break`, `continue` and `return` close an iterator without any of them
/// learning what an iterator is.
#[derive(Clone)]
enum Cleanup {
    /// A `finally` block, re-emitted at each exit path.
    Finally(Vec<Statement>),
    /// `if (it.return) it.return()` against a hidden `for-of` binding. Held as the binding rather
    /// than as statements because it has no source form to re-compile.
    CloseIterator {
        binding: String,
        awaiting: bool,
        span: Span,
    },
    /// The `using` declarations of one scope, disposed in reverse declaration order.
    ///
    /// Held as bindings rather than as a runtime list because which declarations a scope
    /// contains is a fact about its text: the *number* of resources is static even though
    /// whether each one was reached is not, and a pair of hidden bindings per declaration says
    /// both at once -- an empty method binding is a declaration this pass never got to.
    DisposeResources(Vec<ResourceSlot>),
}

/// Where one `using` declaration keeps what has to be disposed of, and how.
#[derive(Clone)]
struct ResourceSlot {
    /// The resource itself, which is the receiver its disposal is called against.
    value: String,
    /// The method, read once where the declaration is. Reading it again at the end of the scope
    /// would run an accessor twice and could find a different function the second time.
    method: String,
    /// `await using`: the disposal is awaited.
    awaiting: bool,
}

/// The `using` declarations of the scope being compiled, and how many have been reached.
struct ResourceScope {
    slots: Vec<ResourceSlot>,
    /// Declarations are handed their slots in source order, which is the order the disposal
    /// walks backwards.
    next: usize,
}

/// What a `break` or a `continue` is allowed to leave.
///
/// A `switch` catches `break` and is transparent to `continue`, so the two jumps do not always
/// choose the same enclosing construct and the stack has to record which is which.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum BreakableKind {
    #[default]
    Loop,
    Switch,
    /// A labelled statement that is not a loop. `break label` leaves it; `continue label` names
    /// something that cannot be continued and is refused.
    Labelled,
}

#[derive(Default)]
struct LoopContext {
    kind: BreakableKind,
    /// Every stacked label written on this construct. `break outer` or `continue outer` can
    /// name any label in this list when the labels share a single loop/switch target.
    labels: Vec<String>,
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// How many cleanups were pending when the loop began, so a `break` runs only those entered
    /// inside it.
    finalizer_depth: usize,
    /// How many exception handlers were active when the loop began. A `break` out of a `try`
    /// inside the body has to pop the ones it is jumping past, or they outlive their region and
    /// catch something that is not theirs.
    handler_depth: usize,
}

#[derive(Clone, Copy)]
struct Local {
    slot: u32,
    kind: VariableKind,
    imported: bool,
    /// False between the point a block's bindings are created and the point this one's
    /// declaration runs -- the temporal dead zone.
    ///
    /// Every name a block declares exists from the moment the block is entered, because a
    /// function declared at the top of it can be called immediately and its body may refer to a
    /// `const` written further down. The runtime marker catches a direct read before the
    /// declaration; a nested function resolves the same name as an upvalue and checks the shared
    /// cell when it eventually runs.
    initialized: bool,
    /// The slot holds a cell rather than the value, because a nested function refers to this
    /// name. Reads and writes go through `LoadCell`/`StoreCell` so every closure sharing the
    /// binding sees the same mutations.
    celled: bool,
    /// The eval binding category when this is an outer binding imported into an eval compiler,
    /// or the private name of a named function expression. It preserves distinctions that
    /// `VariableKind` alone cannot express.
    eval_declaration: Option<EvalDeclaration>,
}

/// The per-function half of [`Compiler`], saved and restored around a nested function body.
///
/// Constants and diagnostics are deliberately not in here: they are program-wide, and splitting
/// them would mean merging them back with index fixups for no benefit.
struct FunctionState {
    code: Vec<Instruction>,
    scopes: Vec<HashMap<String, Local>>,
    local_count: u32,
    upvalues: Vec<Upvalue>,
    /// Names already captured, so a name referenced twice captures one cell rather than two.
    upvalue_names: HashMap<String, u32>,
    /// Names this function's nested functions refer to, so declarations know to box themselves.
    captured: HashSet<String>,
    /// Whether every binding of this function is boxed, because a direct `eval` can see them.
    cell_everything: bool,
    active_function_span: Option<kiln_hir::Span>,
    current_function_span: Option<kiln_hir::Span>,
}

/// Where a name lives, which decides how it is read and whether it can be read at all.
/// One active `with`, and where it sits so a binding written inside it can shadow it.
#[derive(Clone, Debug)]
struct WithScope {
    /// The hidden binding holding the object. It is celled, because a function created inside
    /// the body still resolves its names against the object when it is called later.
    name: String,
    /// How many functions deep the `with` itself is, so a nested function can tell that its own
    /// parameters and locals are inside the object environment rather than outside it.
    functions: usize,
    /// How many scopes were open when the body started. A binding declared at or beyond this is
    /// nested *within* the object environment record and therefore shadows it.
    scopes: usize,
}

enum Resolution {
    /// A binding in the function currently being compiled.
    Local(Local),
    /// A module-scope binding, reached from inside a call frame.
    Global(u32),
    /// A binding in an enclosing *function*. That is a closure capture, and compiling it as
    /// either of the above would read an unrelated slot -- silently, and with a plausible value
    /// in it. Refused until upvalues exist.
    /// A binding in an enclosing function, reached through this closure's captured cells.
    Upvalue(u32),
    /// A `var` or function declaration at the top level of *global* code, which the
    /// specification puts on the global object rather than in an environment slot.
    ///
    /// It is reached by name, because the object is where it lives: `var x = 1` at the top of a
    /// script and `globalThis.x = 1` are two spellings of one property, and every later read --
    /// from the script, from a function it declares, from an `eval` written anywhere -- has to
    /// find the same one.
    GlobalObject,
    Unresolved,
}

/// The private methods and instance fields a class installs on every instance it builds.
///
/// Carried rather than emitted in place because *when* they are installed is part of the
/// language: a base class installs them on entry to its constructor, and a derived class
/// installs them the moment `super()` returns -- which is the moment `this` first exists, and,
/// when the parent is an exotic builtin, the moment it stops being the object the derived class
/// allocated.
#[derive(Debug, Clone)]
struct InstanceElements {
    private: Vec<(String, PrivateKind, String)>,
    fields: Vec<(ClassField, Option<String>)>,
}

/// What makes a constructor a *class* constructor rather than an ordinary function.
///
/// A constructor is a function plus the instance elements it owes and the place it owes them, so
/// this is the whole of the difference: `compile_constructor` takes `None` for a plain function
/// and one of these for a class body.
struct ClassBody<'a> {
    /// The class extends something, so `this` does not exist until `super()` has run.
    derived: bool,
    /// A derived class with no constructor of its own, whose synthesized body is
    /// `super(...arguments)` and nothing else.
    forwards_to_super: bool,
    private_elements: &'a [(String, PrivateKind, String)],
    fields: &'a [(&'a ClassField, Option<String>)],
}

impl ClassBody<'_> {
    /// Whether the elements have to wait for a `super()` the body writes for itself.
    ///
    /// A base class has no `super()` to wait for, and a synthesized forward has already made
    /// the call by the time the prologue ends -- so only a derived class with a constructor of
    /// its own defers.
    fn installs_elements_at_super(&self) -> bool {
        self.derived && !self.forwards_to_super
    }

    fn elements(&self) -> InstanceElements {
        InstanceElements {
            private: self.private_elements.to_vec(),
            fields: self
                .fields
                .iter()
                .map(|(field, binding)| ((*field).clone(), binding.clone()))
                .collect(),
        }
    }
}

struct Compiler {
    /// The text being compiled, handed to the [`Program`] so a function can answer with its own
    /// definition. Shared, never copied: see [`Program::source`].
    source: Arc<str>,
    /// Original-source mapping for statically transformed TypeScript. Function spans still index
    /// `source`; this sidecar is used only for source-facing diagnostics and call-site locations.
    source_provenance: Option<Arc<kiln_hir::StaticSourceProvenance>>,
    /// Lossless source units and parser-byte mapping when this source used a transport encoding.
    source_origin: Option<Arc<kiln_hir::SourceOrigin>>,
    constants: Vec<Constant>,
    /// Lookup only: insertion order in `constants` remains the bytecode's stable index order.
    constant_indices: rustc_hash::FxHashMap<Constant, u32>,
    functions: Vec<FunctionProto>,
    template_sites: Vec<TemplateSite>,
    debug_call_sites: Vec<(Option<kiln_hir::Span>, u32, kiln_hir::Span)>,
    active_function_span: Option<kiln_hir::Span>,
    /// Whether `functions[i]` was compiled as strict code, in the same order.
    ///
    /// Beside the protos rather than on them, because a proto is bytecode the C backend reads
    /// too and this is the interpreter's answer to one question: whether a refused write throws
    /// or is discarded. See [`Strictness`].
    strict_functions: Vec<bool>,
    removable_declarations: Vec<RemovableDeclaration>,
    module_instantiation_end: u32,
    /// Enclosing function states, module body first. Empty while compiling the module body.
    enclosing: Vec<FunctionState>,
    upvalues: Vec<Upvalue>,
    upvalue_names: HashMap<String, u32>,
    captured: HashSet<String>,
    /// Whether the code being compiled is strict.
    ///
    /// Recorded rather than derived, because a directive prologue does not survive lowering: the
    /// goal and the harness variant are what say so, and they are the caller's to pass in. Only
    /// `eval` reads it -- a direct eval in strict code gets its own scope for its declarations,
    /// and in sloppy code shares the caller's.
    strict: bool,
    /// The local holding the completion value, while compiling an eval body's statements.
    ///
    /// `Some` only for the top level of an eval program: a value produced inside a function is
    /// that function's business, and `enclosing` being non-empty is what says we are in one.
    completion: Option<u32>,
    /// Whether this compilation is an `eval` program rather than a file.
    ///
    /// The one thing it changes is the temporal dead zone. Both files and eval code preserve a
    /// reachable dead-zone read for runtime, because the language promises a `ReferenceError` --
    /// `assert.throws(ReferenceError, () => eval("typeof x; let x;"))` -- rather than rejecting
    /// source that may never reach the read. Every eval binding is boxed, so the runtime check is
    /// already there to raise it.
    eval_program: bool,
    /// Whether this compilation is a *module* body rather than a script or an `eval`.
    ///
    /// Only `await` reads it. A module body may await, and doing so makes its evaluation
    /// asynchronous -- the body stops and the host resumes it; a script has nowhere to put that
    /// suspension, and `await` there is a plain identifier or a syntax error depending on the
    /// context, never a suspension.
    module_body: bool,
    /// Whether the code being compiled is one parameter's default, with later parameters still
    /// unbound.
    ///
    /// The parameter list is its own scope, and every name in it exists -- uninitialised --
    /// from the moment the call begins. So `function f(x = x) {}` compiles and only *calling*
    /// it fails, and the failure is a `ReferenceError`, not a rejection of the source. The flag
    /// is what tells a dead-zone read here apart from one in a body, where the same runtime check
    /// must remain reachable so a surrounding `try` can catch it.
    parameter_dead_zone: bool,
    /// The names the parameter environment currently being evaluated holds, when there is one.
    ///
    /// `Some` only while a parameter list with defaults is being compiled, and cleared before
    /// the body -- because a body's variable environment *is* the one an eval written there
    /// declares into, and only a parameter list's is a separate record between them. Saved and
    /// restored per function, so an `eval` in a function nested inside a default sees its own
    /// context rather than the enclosing list's.
    parameter_scope: Option<Vec<String>>,
    /// Names a top-level `var` in *this* eval program may not create, from its call site.
    ///
    /// Only ever non-empty for sloppy eval code, and the reason is the whole rule: strict eval
    /// gets a variable environment of its own, so it has nothing to collide with.
    blocked_var_names: Vec<String>,
    /// Names of the calling frame's bindings borrowed into scope 0 of an eval program.
    ///
    /// Tracked apart from the scope itself because a declaration in the eval'd source has to
    /// tell a borrowed binding from one of its own: a sloppy `var` takes the caller's over, and
    /// everything else shadows it.
    outer_names: HashSet<String>,
    /// Source spans of the block-level declarations Annex B B.3.2.1/B.3.3.2 gives a var-scoped
    /// binding in the body being compiled.
    ///
    /// `{ function f() {} }` is two bindings, not one: a block-scoped one, which is all the main
    /// grammar creates, and -- in sloppy code -- a var-scoped one in the enclosing function or
    /// script, which the declaration writes to as it is evaluated. Legacy, and load-bearing:
    /// bundled JavaScript still relies on it. Names that are blocked -- by a lexical declaration
    /// between here and the block, by a formal parameter, or by being `arguments` -- are never in
    /// this set. Declaration identity matters: an eligible sibling with the same name must not
    /// authorize writes from a blocked declaration.
    annexb_var_functions: HashSet<Span>,
    /// Names this program declared on the global object rather than in a slot.
    ///
    /// Non-empty only for *global* code: a script and an `eval` whose variable environment is the
    /// global one. A module body and a CommonJS body each have a variable environment of their
    /// own, and their top-level `var`s stay slots -- which is the difference between the two
    /// things kiln parses with the script grammar.
    global_bindings: HashSet<String>,
    /// Whether this program's variable environment *is* the global environment record, so that a
    /// top-level `var` or function declaration is a property of the global object.
    global_var_scope: bool,
    /// Whether a binding this program creates on the global object may be deleted afterwards.
    ///
    /// `false` for a script, whose `CreateGlobalVarBinding` is called with `D` false, and `true`
    /// for an `eval`, whose is called with `D` true. It is the whole reason `var x` at the top of
    /// a script survives `delete x` and one written inside an `eval` does not.
    global_bindings_configurable: bool,
    /// Box every binding of the function being compiled, not only the ones a nested function
    /// names.
    ///
    /// Set when a direct `eval` appears anywhere inside this function, including inside a
    /// function nested in it. The eval'd source is compiled separately and reaches the caller's
    /// bindings through shared cells, so a binding it might name has to *be* a cell -- and which
    /// names it will use is unknowable until it runs. Over-approximating costs one pointer chase
    /// per access in a function that already contains an eval, and buys the guarantee that every
    /// name at the call site can be handed over.
    cell_everything: bool,
    /// Loops currently being compiled, innermost last, holding the jumps their `break` and
    /// `continue` statements still need patched.
    loops: Vec<LoopContext>,
    /// Counter for compiler-introduced bindings, so two in one scope never collide.
    hidden_bindings: usize,
    /// Whether the function being compiled would be given a *mapped* `arguments` object:
    /// `IsSimpleParameterList` is true and the code is sloppy.
    ///
    /// Recorded here because the mention of `arguments` is compiled far from the parameter list
    /// that decides it, and only [`Instruction::LoadArguments`] reads it.
    mapped_arguments: bool,
    /// Whether the function being compiled is an async generator.
    ///
    /// `yield*` is the only thing that reads it, and it needs it because the two generator kinds
    /// delegate to *different protocols*: an async `yield*` asks for `@@asyncIterator` and awaits
    /// each step, and reaching for `@@iterator` instead is observable -- a program that defines
    /// both sees the wrong one run, and one that defines only a throwing `@@iterator` getter sees
    /// an exception the specification says can never happen.
    async_generator: bool,
    /// The scopes containing `using` declarations that are being compiled, innermost last.
    ///
    /// A stack because a block with resources can be written inside another, and each
    /// declaration belongs to the scope it was written in rather than to the innermost one open
    /// at the time it runs.
    resource_scopes: Vec<ResourceScope>,
    /// The `with` statements whose bodies are being compiled, outermost first.
    ///
    /// Every name mentioned inside one is looked for on these objects before the bindings around
    /// it, and only code lexically inside a `with` pays for that: the resolution everywhere else
    /// is the static one it has always been.
    with_scopes: Vec<WithScope>,
    /// Eval program top-level lexical names, which shadow inherited object environments.
    eval_lexical_names: HashSet<String>,
    /// `finally` blocks enclosing the code being compiled, innermost last. A non-local exit has
    /// to run each of them on its way out, so it needs their bodies rather than their addresses.
    finalizers: Vec<Cleanup>,
    /// The class whose private names are in scope, innermost last.
    ///
    /// A stack because a class can be declared inside another's method, and each has its own
    /// private names.
    private_scopes: Vec<u32>,
    /// The source module that owns each scope, parallel to `private_scopes`. Class ordinals are
    /// local to one compiler; these identities keep their reserved property keys disjoint when
    /// a graph compiles each module independently.
    private_scope_modules: Vec<String>,
    /// Stable identity of the module currently being compiled, used for classes declared here.
    private_module_id: String,
    /// The hidden binding holding each open class's *brand*: the identity that tells one
    /// evaluation of a class from another. `private_scopes` distinguishes one class body from
    /// another and is fixed at compile time; two evaluations of one body share it, which is why
    /// the brand cannot be part of the key.
    private_brands: Vec<String>,
    /// The next unused class identity for `private_scopes`.
    next_private_scope: u32,
    /// The private names each of those classes declares, innermost last.
    ///
    /// A private name is resolved lexically, like any other binding: `Outer.#x` written inside a
    /// class nested in `Outer` names *Outer's* element. Keying every mention to the innermost
    /// class made the nested mention a reservation no object carries, which the brand check then
    /// reported as a class that did not declare its own field.
    private_declarations: Vec<HashSet<String>>,
    /// Scratch used while re-emitting cleanups, so the stack is restored afterwards.
    pending_finalizers: Vec<Cleanup>,
    /// Labels written immediately before the statement now being compiled, waiting for that
    /// statement to claim them. A loop/switch takes the stack into its context; anything else
    /// gets a labelled block around each label.
    pending_labels: Vec<String>,
    /// Short-circuit jumps of the optional chains being compiled, innermost last.
    ///
    /// A stack rather than a field because a chain can appear inside another chain's arguments,
    /// and each `?.` has to leave the chain it was written in rather than the innermost one.
    chains: Vec<Vec<usize>>,
    /// Exception handlers active at this point in the emitted code. Tracked because a non-local
    /// exit jumps past `PopHandler` instructions that would otherwise have balanced them.
    handler_depth: usize,
    /// Cleanups pending when the current function began. A `return` unwinds to here and no
    /// further: a `finally` outside the function is not left by returning from it.
    function_finalizer_depth: usize,
    /// The hidden `super` bindings of the derived classes currently being compiled, innermost
    /// last. A stack rather than a field because class bodies nest.
    super_bindings: Vec<String>,
    /// The instance elements each constructor currently being compiled still owes its instance,
    /// innermost last, and `None` for a constructor that has already emitted them.
    ///
    /// A *derived* class initialises its fields when `super()` returns, not when the constructor
    /// is entered, and the difference is visible whenever the parent hands back a different
    /// object than the one allocated up front -- `class E extends Error { name = "E" }` with an
    /// explicit constructor wrote its field onto the abandoned instance and read `"Error"` back.
    /// So the elements are carried here and emitted at each `super()` site instead.
    pending_elements: Vec<Option<InstanceElements>>,
    /// Monotonic, so two derived classes in one scope never share a binding. The stack depth
    /// cannot serve: it returns to zero after each class, and the bindings are siblings in the
    /// same scope rather than nested.
    super_binding_count: usize,
    /// The hidden `[[HomeObject]]` bindings of the methods currently being compiled, innermost
    /// last.
    ///
    /// `super.x` is a lookup on the prototype of the object the *method* was defined on, which
    /// is a different object from the parent constructor: `C.prototype` for an instance method,
    /// `C` itself for a static one, and the literal for a method written in one. Resolving it
    /// through the superclass binding instead answered `Parent.prototype` for all three -- so a
    /// static method read the wrong object, and a method in a class with no `extends`, or in an
    /// object literal, could not be compiled at all because there was no superclass binding to
    /// read. A binding of its own, resolved exactly the way `%super` is, gives every one of them
    /// the object the specification names.
    home_bindings: Vec<String>,
    /// Monotonic, for the reason `super_binding_count` is.
    home_binding_count: usize,
    /// Whether the code being compiled sits inside a function body at all.
    ///
    /// Not `!enclosing.is_empty()`: an arrow written at the top level of a script pushes an
    /// enclosing state and is still outside every function for the one question this answers,
    /// which is whether `new.target` may be written. An arrow inherits this from where it was
    /// written; every other function sets it.
    in_function: bool,
    /// Whether `super.x` is legal at the point being compiled.
    ///
    /// `home_bindings` cannot answer this on its own: it is a compiler-wide stack, so an
    /// ordinary function nested in a method still sees the method's home object even though the
    /// language makes `super.x` inside it a `SyntaxError`. This is set from
    /// [`Compiler::next_is_method`] at each function boundary and inherited by arrows.
    super_property: bool,
    /// Set by a caller immediately before it compiles a method's body, and taken by that body.
    ///
    /// One-shot rather than a stack because it is read exactly once, at the function boundary it
    /// was set for: everything nested inside then reads `super_property` instead.
    next_is_method: bool,
    /// The hidden binding holding `this` while a *derived* constructor runs, if that is what is
    /// being compiled.
    ///
    /// A derived constructor's `this` does not exist until `super()` returns -- reading it before
    /// then is a `ReferenceError`, and so is returning without having called `super()` at all.
    /// Kiln allocates the instance up front, so there was no moment at which `this` was absent
    /// and every one of those reads answered with a half-built object instead.
    ///
    /// A binding rather than a flag on the frame, because the check it needs is exactly the
    /// temporal dead zone every `let` already has: an uninitialised cell, read through
    /// `LoadCell`. That costs no new instruction, and -- what matters more -- the C backend
    /// refuses nothing and implements nothing new, so the two cannot disagree about it.
    this_binding: Option<String>,
    /// Whether `return` belongs to a derived constructor itself.
    ///
    /// Kept separate from `this_binding` because an arrow nested in a derived constructor
    /// inherits that binding for lexical `this` and `super()`, but remains an ordinary function:
    /// `return 1` returns `1` from the arrow and falling off its end returns `undefined`.
    derived_constructor: bool,
    /// Monotonic, for the reason `home_binding_count` is.
    this_binding_count: usize,
    /// Whether the code being compiled is a class field initialiser or a `static {}` block.
    ///
    /// Kiln emits an instance field's initialiser straight into the constructor and a static
    /// field's into the class definition, so neither has a function boundary of its own -- but
    /// the language gives both one for `new.target`, which is `undefined` in an initialiser
    /// whatever the constructor was called with.
    field_initialiser: bool,
    code: Vec<Instruction>,
    /// Lexical scopes of the function being compiled, innermost last. At module level the first
    /// entry is module scope itself.
    scopes: Vec<HashMap<String, Local>>,
    /// Slots are never recycled: bytecode can jump back into a block in a loop, and a stable
    /// slot keeps that execution independent of compiler scope bookkeeping.
    local_count: u32,
    diagnostics: Vec<CompileDiagnostic>,
    pub(super) current_function_span: Option<Span>,
    pub(super) local_spans: BTreeMap<(Option<Span>, u32), Span>,
    pub(super) new_object_spans: BTreeMap<(Option<Span>, u32), Span>,
    pub(crate) new_array_spans: BTreeMap<(Option<Span>, u32), Span>,
}

mod bindings;
mod classes;
mod compiler_setup;
mod expressions;
mod statements;

/// Module-scope names whose value can change after the module body has run.
///
/// A `let` or `var` declaration, or any name something assigns to or updates -- including inside
/// a nested function, which is the case that matters: `export let count` incremented by an
/// exported `bump()` is the whole reason a live binding is not the same as a snapshot.
/// Whether each `using` declaration written directly in this statement list is awaited.
///
/// Only this level: a `using` inside a nested block belongs to that block's scope and is
/// disposed of when *it* is left, which is what makes the disposal order the reverse of the
/// declaration order rather than of the execution order.
mod analysis;
use analysis::*;
pub use analysis::{
    body_contains_direct_eval, body_contains_new_function, collect_dynamic_imports,
    function_contains_direct_eval, function_contains_new_function, immutable_module_bindings,
    module_contains_direct_eval, module_contains_new_function, mutable_module_bindings,
};

pub type BodySpanInfo = (
    BTreeMap<(Option<Span>, u32), Span>,
    BTreeMap<(Option<Span>, u32), Span>,
    BTreeMap<(Option<Span>, u32), Span>,
);

/// Collect the source spans of every local variable slot and every `NewObject` instruction
/// for both top-level code (keyed by `None`) and every function (keyed by `Some(function_span)`).
pub fn collect_body_spans(module: &Module) -> BodySpanInfo {
    let mut compiler = Compiler::new(module);
    if matches!(module.source_goal, SourceGoal::Module) || !module.imports.is_empty() {
        compiler.strict = true;
        compiler.module_body = true;
        for import in module.imports.iter().filter(|import| !import.type_only) {
            compiler.declare_import(&import.local, import.span);
        }
        for (index, export) in module.exports.iter().enumerate() {
            if export.type_only {
                continue;
            }
            if matches!(
                export.origin,
                ExportOrigin::ReExport { .. }
                    | ExportOrigin::NamespaceReExport { .. }
                    | ExportOrigin::Star { .. }
            ) {
                let local = format!("{REEXPORT_LOCAL_PREFIX}{index}*");
                compiler.declare_import(&local, export.span);
            }
        }
        for export in &module.exports {
            match &export.origin {
                ExportOrigin::Local { local } => {
                    compiler.captured.insert(local.clone());
                }
                ExportOrigin::DefaultExpression => {
                    compiler
                        .captured
                        .insert(kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
                }
                _ => {}
            }
        }
    } else {
        compiler.strict = module.strict;
        compiler.global_var_scope = true;
        compiler.global_bindings_configurable = false;
    }
    compiler.compile_body(module);
    (
        compiler.local_spans,
        compiler.new_object_spans,
        compiler.new_array_spans,
    )
}

/// Collect body spans matching the program's top-level code layout, accounting for whether
/// the program was compiled with hoisted module body functions or sloppy script statements.
pub(crate) fn collect_body_spans_for_program(module: &Module, program: &Program) -> BodySpanInfo {
    let mut compiler = Compiler::new(module);
    compiler.strict = true;
    compiler.module_body = true;
    for import in module.imports.iter().filter(|import| !import.type_only) {
        compiler.declare_import(&import.local, import.span);
    }
    for (index, export) in module.exports.iter().enumerate() {
        if export.type_only {
            continue;
        }
        if matches!(
            export.origin,
            ExportOrigin::ReExport { .. }
                | ExportOrigin::NamespaceReExport { .. }
                | ExportOrigin::Star { .. }
        ) {
            let local = format!("{REEXPORT_LOCAL_PREFIX}{index}*");
            compiler.declare_import(&local, export.span);
        }
    }
    for export in &module.exports {
        match &export.origin {
            ExportOrigin::Local { local } => {
                compiler.captured.insert(local.clone());
            }
            ExportOrigin::DefaultExpression => {
                compiler
                    .captured
                    .insert(kiln_hir::DEFAULT_EXPORT_LOCAL.to_owned());
            }
            _ => {}
        }
    }
    compiler.compile_body(module);
    let ok = compiler.code.len() == program.code.len()
        && compiler.new_object_spans.keys().all(|(func_idx, pc)| {
            if func_idx.is_none() {
                matches!(
                    program.code.get(*pc as usize),
                    Some(Instruction::NewObject { .. } | Instruction::Nop)
                )
            } else {
                true
            }
        })
        && compiler.new_array_spans.keys().all(|(func_idx, pc)| {
            if func_idx.is_none() {
                matches!(
                    program.code.get(*pc as usize),
                    Some(Instruction::NewArray { .. } | Instruction::Nop)
                )
            } else {
                true
            }
        });
    if ok {
        (
            compiler.local_spans,
            compiler.new_object_spans,
            compiler.new_array_spans,
        )
    } else {
        collect_body_spans(module)
    }
}

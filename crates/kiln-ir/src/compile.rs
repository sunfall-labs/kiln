use super::*;
use kiln_hir::{
    AssignmentOperator, BinaryOperator, CatchClause, Class, ClassField, ClassInitialiser,
    ExportName, ExportOrigin, Expression, ExpressionKind, Function, ImportName, ListElement,
    Literal, LogicalOperator, Module, ObjectKey, ObjectMember, Pattern, PatternKey, SourceGoal,
    Statement, StatementKind, SwitchCase, UnaryOperator, VariableKind,
};
use std::collections::{HashMap, HashSet};
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
                    EvalDeclaration::Const => VariableKind::Const,
                },
                imported: false,
                initialized: true,
                // The caller's cell is installed in the slot before the body runs, so there is
                // nothing for a prologue to box and every access goes straight through it.
                celled: true,
            },
        );
    }
    // The enclosing classes' private names, so `eval("this.#m")` resolves `#m` to the element
    // the class actually declared rather than to a key of its own. The identities come with them
    // for the reason `PrivateScope` records, and the counter is advanced past every one: a class
    // declared *inside* this eval must not be handed an identity the caller is already using, or
    // its brand check would accept the caller's instances.
    for scope in &options.private_scopes {
        compiler.private_scopes.push(scope.scope);
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
/// indirect re-exports, default expression exports, and cycles remain explicit unsupported forms.
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
            star_exports.push(ExportBinding {
                exported: format!("{STAR_REEXPORT_PREFIX}{source}"),
                local_slot,
            });
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

    // An exported binding that can *change* is boxed, so the importer holds this module's cell
    // rather than a copy of the value it had when the module finished. One that cannot change --
    // a `const`, a function declaration, a class -- is left as a plain slot: a snapshot and a
    // live view of it are indistinguishable, and boxing it would cost the native backend, which
    // has no cells, a module it can otherwise compile.
    let mutable = mutable_module_bindings(&module.body);
    for export in &module.exports {
        if let ExportOrigin::Local { local } = &export.origin
            && mutable.contains(local)
        {
            compiler.captured.insert(local.clone());
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
        node_runtime_requests: Vec::new(),
        immutable_bindings: immutable_module_bindings(module),
        direct_eval: body_contains_direct_eval(&module.body),
        new_function: body_contains_new_function(&module.body),
        dynamic_imports: collect_dynamic_imports(&module.body),
        removable_declarations: compiler.removable_declarations.clone(),
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

/// Compile a CommonJS body with Node's wrapper bindings installed as module-scope cells.
///
/// The graph supplies the five values in this exact order. They are ordinary mutable bindings:
/// `module.exports = value` must be observable after evaluation, and nested functions capture
/// `require`/`exports` through the same cell mechanism as any other module-scope name.
pub fn compile_commonjs(module: &Module) -> Result<ModuleProgram, Vec<CompileDiagnostic>> {
    const WRAPPER: [&str; 5] = ["exports", "require", "module", "__filename", "__dirname"];
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
        node_runtime_requests: Vec::new(),
        immutable_bindings: immutable_module_bindings(module),
        direct_eval: body_contains_direct_eval(&module.body),
        new_function: body_contains_new_function(&module.body),
        dynamic_imports: collect_dynamic_imports(&module.body),
        removable_declarations: Vec::new(),
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
    CloseIterator { binding: String },
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
    /// The label written on this construct, if any. `break outer` names one of these.
    label: Option<String>,
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
    constants: Vec<Constant>,
    functions: Vec<FunctionProto>,
    template_sites: Vec<TemplateSite>,
    /// Whether `functions[i]` was compiled as strict code, in the same order.
    ///
    /// Beside the protos rather than on them, because a proto is bytecode the C backend reads
    /// too and this is the interpreter's answer to one question: whether a refused write throws
    /// or is discarded. See [`Strictness`].
    strict_functions: Vec<bool>,
    removable_declarations: Vec<RemovableDeclaration>,
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
    /// Names of the block-level function declarations Annex B B.3.2.1/B.3.3.2 gives a var-scoped
    /// binding in the body being compiled.
    ///
    /// `{ function f() {} }` is two bindings, not one: a block-scoped one, which is all the main
    /// grammar creates, and -- in sloppy code -- a var-scoped one in the enclosing function or
    /// script, which the declaration writes to as it is evaluated. Legacy, and load-bearing:
    /// bundled JavaScript still relies on it. Names that are blocked -- by a lexical declaration
    /// between here and the block, by a formal parameter, or by being `arguments` -- are never in
    /// this set, so the write is emitted only where the specification asks for it.
    annexb_var_functions: HashSet<String>,
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
    /// `finally` blocks enclosing the code being compiled, innermost last. A non-local exit has
    /// to run each of them on its way out, so it needs their bodies rather than their addresses.
    finalizers: Vec<Cleanup>,
    /// The class whose private names are in scope, innermost last.
    ///
    /// A stack because a class can be declared inside another's method, and each has its own
    /// private names.
    private_scopes: Vec<u32>,
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
    /// A label written immediately before the statement now being compiled, waiting for that
    /// statement to claim it. A loop takes it into its own context; anything else gets a
    /// labelled block around it.
    pending_label: Option<String>,
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
}

impl Compiler {
    fn set_property_instruction(&self, name: u32) -> Instruction {
        Instruction::SetPropertyStrict {
            name,
            strict: self.strict,
        }
    }

    fn set_computed_instruction(&self) -> Instruction {
        Instruction::SetComputedStrict {
            strict: self.strict,
        }
    }

    fn set_super_property_instruction(&self, name: u32) -> Instruction {
        Instruction::SetSuperPropertyStrict {
            name,
            strict: self.strict,
        }
    }

    fn set_super_element_instruction(&self) -> Instruction {
        Instruction::SetSuperElementStrict {
            strict: self.strict,
        }
    }

    fn delete_property_instruction(&self, name: u32) -> Instruction {
        Instruction::DeletePropertyStrict {
            name,
            strict: self.strict,
        }
    }

    fn delete_computed_instruction(&self) -> Instruction {
        Instruction::DeleteComputedStrict {
            strict: self.strict,
        }
    }

    fn new(module: &Module) -> Self {
        let mut compiler = Self {
            source: Arc::clone(&module.source),
            constants: Vec::new(),
            code: Vec::new(),
            functions: Vec::new(),
            template_sites: Vec::new(),
            strict_functions: Vec::new(),
            removable_declarations: Vec::new(),
            enclosing: Vec::new(),
            upvalues: Vec::new(),
            upvalue_names: HashMap::new(),
            // The module body has nested functions of its own, so its block bindings can be
            // captured. Computed here rather than in each entry point, because there are two --
            // `compile` and `compile_module` -- and one of them would have been forgotten.
            captured: names_captured_by_nested_functions(&module.body),
            strict: false,
            completion: None,
            eval_program: false,
            module_body: false,
            parameter_dead_zone: false,
            parameter_scope: None,
            blocked_var_names: Vec::new(),
            outer_names: HashSet::new(),
            annexb_var_functions: HashSet::new(),
            global_bindings: HashSet::new(),
            global_var_scope: false,
            global_bindings_configurable: false,
            // Module scope is boxed too when the module contains a direct eval: a script's
            // top-level bindings are exactly the ones an eval written there can see.
            cell_everything: body_contains_direct_eval(&module.body),
            loops: Vec::new(),
            hidden_bindings: 0,
            resource_scopes: Vec::new(),
            async_generator: false,
            // The module body has no parameters and no `arguments`; the flag is a function's.
            mapped_arguments: false,
            with_scopes: Vec::new(),
            private_scopes: Vec::new(),
            private_brands: Vec::new(),
            next_private_scope: 0,
            private_declarations: Vec::new(),
            finalizers: Vec::new(),
            pending_finalizers: Vec::new(),
            function_finalizer_depth: 0,
            handler_depth: 0,
            pending_label: None,
            chains: Vec::new(),
            super_bindings: Vec::new(),
            pending_elements: Vec::new(),
            super_binding_count: 0,
            home_bindings: Vec::new(),
            home_binding_count: 0,
            in_function: false,
            super_property: false,
            this_binding: None,
            derived_constructor: false,
            this_binding_count: 0,
            next_is_method: false,
            field_initialiser: false,
            scopes: vec![HashMap::new()],
            local_count: 0,
            diagnostics: Vec::new(),
        };
        if module.panicked {
            compiler.error(None, "frontend could not produce an executable module");
        }
        for diagnostic in &module.diagnostics {
            compiler.diagnostics.push(CompileDiagnostic {
                message: diagnostic.message.clone(),
                span: diagnostic.span,
            });
        }
        compiler
    }

    fn compile_body(&mut self, module: &Module) {
        // A module body is a variable environment like a function's, so its `var`s are hoisted
        // to the top of it rather than to the top of whichever block they were written in.
        self.hoist_var_names(&module.body);
        // Script code, so B.3.3.2 rather than B.3.2.1: there are no parameters to decline for.
        // A module body is strict and `hoist_annexb_function_names` returns immediately for it.
        self.hoist_annexb_function_names(&module.body, &HashSet::new());
        if self.module_body {
            self.plain_module_statements(&module.body);
        } else {
            self.plain_statements(&module.body);
        }
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Return);
    }

    fn plain_module_statements(&mut self, list: &[Statement]) {
        for statement in list {
            self.hoist_names(statement);
        }
        for statement in list {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        for statement in list {
            if matches!(statement.kind, StatementKind::Function(_)) {
                continue;
            }
            let removable_names = removable_declaration_names(statement);
            let pc_start = self.code.len();
            self.statement(statement);
            if let Some(names) = removable_names {
                let local_slots = names
                    .iter()
                    .filter_map(|name| self.resolve_module_local(name))
                    .collect::<Vec<_>>();
                if local_slots.len() == names.len() {
                    self.removable_declarations.push(RemovableDeclaration {
                        local_slots,
                        pc_start: pc_start as u32,
                        pc_end: self.code.len() as u32,
                    });
                }
            }
        }
    }

    /// The body of an eval program, which returns its completion value.
    ///
    /// `eval("1 + 1")` is 2, and a module body is not: it always completes with `undefined`, so
    /// the ordinary path throws every statement's value away. The completion value is the last
    /// *expression* statement's, held in a slot of its own so that a statement which produces
    /// nothing -- a declaration, a loop -- leaves the previous one standing, which is what the
    /// specification's completion records do.
    ///
    /// Tracked at every depth, because a completion value is usually produced below the top
    /// level: `eval("2; switch (1) { case 1: 3; break; }")` is 3, and `eval("1; if (true) { }")`
    /// is `undefined` rather than 1. See `reset_completion` for which statements normalise an
    /// empty completion and which pass one through.
    fn compile_eval_body(&mut self, module: &Module) {
        let completion = self.local_count;
        self.local_count += 1;
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::StoreLocal(completion));
        // The same three passes `statements` makes -- names, then hoisted function values, then
        // the rest -- with the expression statements diverted into the completion slot.
        self.hoist_var_names(&module.body);
        self.hoist_annexb_function_names(&module.body, &HashSet::new());
        for statement in &module.body {
            self.hoist_names(statement);
        }
        for statement in &module.body {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        self.completion = Some(completion);
        for statement in &module.body {
            if !matches!(statement.kind, StatementKind::Function(_)) {
                self.statement(statement);
            }
        }
        self.completion = None;
        self.code.push(Instruction::LoadLocal(completion));
        self.code.push(Instruction::Return);
    }

    fn finish_with_strictness(self) -> Result<(Program, Strictness), Vec<CompileDiagnostic>> {
        if self.diagnostics.is_empty() {
            Ok((
                Program {
                    version: BYTECODE_VERSION,
                    constants: self.constants,
                    code: self.code,
                    local_count: self.local_count,
                    functions: self.functions,
                    source: self.source,
                    template_sites: self.template_sites,
                },
                Strictness {
                    body: self.strict,
                    functions: self.strict_functions,
                },
            ))
        } else {
            Err(self.diagnostics)
        }
    }

    fn declare_import(&mut self, name: &str, span: Span) -> Option<u32> {
        if self.scopes[0].contains_key(name) {
            self.error(Some(span), format!("duplicate module binding `{name}`"));
            return None;
        }
        let slot = self.local_count;
        self.local_count += 1;
        self.scopes[0].insert(
            name.to_owned(),
            Local {
                slot,
                kind: VariableKind::Const,
                imported: true,
                // An import is bound before the module body runs, so it is never in a dead zone.
                initialized: true,
                // *Always* a cell. An import is a live view of the exporter's binding, not a
                // copy of its value at the moment the exporter finished -- `export let count`
                // that a function later increments has to read as the new number here. The cell
                // is the exporter's own, installed by the host before the first instruction, so
                // there is nothing for the module prologue to box.
                celled: true,
            },
        );
        Some(slot)
    }

    /// The reserved key a private field is stored under, in the class being compiled.
    ///
    /// Scoped to the class rather than to the name: `class A { #v } class B { #v }` declare two
    /// different fields, and `a.#v` on a `B` is a TypeError rather than a read of B's. One key
    /// per name would have made those the same slot, which is a wrong answer -- and the one
    /// `#v in b` would have reported as a match.
    fn private_key(&self, name: &str) -> String {
        // The innermost class that *declares* the name, which is not always the innermost class:
        // a nested class does not shadow a name it never mentioned.
        for (depth, declared) in self.private_declarations.iter().enumerate().rev() {
            if declared.contains(name) {
                return private_key(self.private_scopes[depth], name);
            }
        }
        // Undeclared anywhere. A syntax error in its own right, reported where the name is used
        // rather than invented into some other class's scope.
        private_key(self.private_scopes.last().copied().unwrap_or(0), name)
    }

    /// The brand binding of the class that *declares* `name`, mirroring [`Self::private_key`].
    fn private_brand(&self, name: &str) -> Option<String> {
        for (depth, declared) in self.private_declarations.iter().enumerate().rev() {
            if declared.contains(name) {
                return self.private_brands.get(depth).cloned();
            }
        }
        self.private_brands.last().cloned()
    }

    /// Push a brand resolved earlier, for the access instruction that follows.
    fn push_private_brand(&mut self, brand: &Option<String>) {
        match brand {
            Some(binding) => self.load_captured_binding(binding),
            None => self.push_constant(Constant::Undefined),
        }
    }

    /// Push the brand of the class declaring `name`, for the access instruction that follows.
    fn load_private_brand(&mut self, name: &str) {
        match self.private_brand(name) {
            Some(binding) => self.load_captured_binding(&binding),
            // A private name outside any class is a syntax error reported elsewhere; the access
            // still needs a value here so the stack stays balanced.
            None => self.push_constant(Constant::Undefined),
        }
    }

    /// The reserved name an instance stores its class's brand under.
    ///
    /// One per class, not one per member: the brand says which *evaluation* an instance came
    /// from, and every private member of that class shares the answer.
    fn private_brand_key(&self, name: &str) -> String {
        for (depth, declared) in self.private_declarations.iter().enumerate().rev() {
            if declared.contains(name) {
                return private_key(self.private_scopes[depth], "%brand");
            }
        }
        private_key(self.private_scopes.last().copied().unwrap_or(0), "%brand")
    }

    /// The brand key of the class currently being compiled, for the store that installs it.
    fn own_private_brand_key(&self) -> String {
        private_key(self.private_scopes.last().copied().unwrap_or(0), "%brand")
    }

    fn resolve_module_local(&self, name: &str) -> Option<u32> {
        self.scopes[0].get(name).map(|local| local.slot)
    }

    fn error(&mut self, span: Option<Span>, message: impl Into<String>) {
        self.diagnostics.push(CompileDiagnostic {
            message: message.into(),
            span,
        });
    }

    /// The completion slot, when the statement being compiled can contribute to one.
    ///
    /// Inside a function body it cannot: `enclosing` is non-empty there, the slot index belongs
    /// to another frame's locals, and the value is the function's to return.
    fn completion_slot(&self) -> Option<u32> {
        self.enclosing
            .is_empty()
            .then_some(self.completion)
            .flatten()
    }

    /// `UpdateEmpty(stmtCompletion, undefined)`, which is how most statements end.
    ///
    /// `if`, the five loops, `switch`, `with` and `try` each normalise an empty body completion
    /// to `undefined` rather than passing the previous statement's value along -- so
    /// `eval("1; if (true) { }")` is `undefined`, not 1. Seeding the slot before the body is the
    /// same thing said forwards: whatever the body produces overwrites it, and a body that
    /// produces nothing leaves the `undefined` standing.
    ///
    /// A block and a labelled statement are deliberately *not* in that list. They pass an empty
    /// completion through, which is why `eval("1; { }")` and `eval("1; lbl: { }")` are both 1.
    fn reset_completion(&mut self) {
        if let Some(slot) = self.completion_slot() {
            self.push_constant(Constant::Undefined);
            self.code.push(Instruction::StoreLocal(slot));
        }
    }

    /// Compile `body` with the completion slot detached, and put it back afterwards.
    ///
    /// A `finally` block's completion is discarded unless it is abrupt, so its statements must
    /// not touch the slot: `eval("try { 2; } finally { 3; }")` is 2.
    fn without_completion(&mut self, body: impl FnOnce(&mut Self)) {
        let saved = self.completion.take();
        body(self);
        self.completion = saved;
    }

    fn statement(&mut self, statement: &Statement) {
        // Every statement that normalises an empty completion to `undefined`, named here rather
        // than in nine separate compile functions so the list can be read against the grammar.
        if matches!(
            statement.kind,
            StatementKind::If { .. }
                | StatementKind::While { .. }
                | StatementKind::DoWhile { .. }
                | StatementKind::For { .. }
                | StatementKind::ForIn { .. }
                | StatementKind::ForOf { .. }
                | StatementKind::Switch { .. }
                | StatementKind::With { .. }
                | StatementKind::Try { .. }
        ) {
            self.reset_completion();
        }
        match &statement.kind {
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    // A plain name keeps the direct path, which knows about `var` redeclaration
                    // and temporal-dead-zone checks. A pattern evaluates its initialiser once
                    // and binds against the result.
                    match declaration.pattern.as_identifier() {
                        Some(name) => {
                            self.variable(
                                *kind,
                                name,
                                declaration.initializer.as_ref(),
                                declaration.span,
                            );
                            // `using` binds exactly as `const` does; the registration is the
                            // whole of what it adds, and it runs after the binding so that a
                            // disposal method reached through the binding sees it initialised.
                            if let Some(awaiting) = kind.disposal() {
                                self.register_resource(name, awaiting, declaration.span);
                            }
                        }
                        None => {
                            match &declaration.initializer {
                                Some(initializer) => self.expression(initializer),
                                // `const { a } = undefined` throws in JavaScript; there is no
                                // throw from the compiler, so it binds undefined and each
                                // property read yields undefined.
                                None => self.push_constant(Constant::Undefined),
                            }
                            self.bind_pattern(*kind, &declaration.pattern, declaration.span);
                        }
                    }
                }
            }
            StatementKind::Expression(expression) => {
                self.expression(expression);
                // At any depth, not just the top level: `eval("2; switch (1) { case 1: 3; }")`
                // is 3, and the statement that produced it is two blocks down.
                match self.completion_slot() {
                    Some(slot) => self.code.push(Instruction::StoreLocal(slot)),
                    None => self.code.push(Instruction::Pop),
                }
            }
            StatementKind::Block(statements) => {
                self.scopes.push(HashMap::new());
                self.statements(statements);
                self.scopes.pop().expect("block scope was just pushed");
            }
            StatementKind::With { object, body } => {
                self.with_statement(object, body, statement.span);
            }
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => self.if_statement(test, consequent, alternate.as_deref()),
            StatementKind::While { test, body } => self.while_statement(test, body),
            StatementKind::DoWhile { body, test } => self.do_while_statement(body, test),
            StatementKind::Switch {
                discriminant,
                cases,
            } => self.switch_statement(discriminant, cases, statement.span),
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => self.for_statement(
                init.as_deref(),
                test.as_ref(),
                update.as_ref(),
                body,
                statement.span,
            ),
            StatementKind::Labelled { label, body } => self.labelled_statement(label, body),
            StatementKind::ForIn {
                kind,
                name,
                object,
                body,
            } => self.for_in(*kind, name, object, body, statement.span),
            StatementKind::ForOf {
                kind,
                pattern,
                iterable,
                body,
                awaiting,
            } => self.for_of(*kind, pattern, iterable, body, *awaiting, statement.span),
            StatementKind::Throw(value) => {
                self.expression(value);
                self.code.push(Instruction::Throw);
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => self.try_statement(block, handler.as_ref(), finalizer, statement.span),
            StatementKind::Break { label } => self.jump_out(label.as_deref(), true, statement.span),
            StatementKind::Continue { label } => {
                self.jump_out(label.as_deref(), false, statement.span)
            }
            StatementKind::Empty => {}
            StatementKind::Function(function) => {
                self.emit_function_declaration(function, statement.span)
            }
            StatementKind::Return(value) => self.return_statement(value.as_ref(), statement.span),
            StatementKind::Class(class) => self.class_declaration(class),
            // `export default <expression>` has no name in the source, so it binds the reserved
            // one the module record already uses for an anonymous default. Everything after it
            // -- the export table, the linker -- then sees an ordinary local export.
            StatementKind::ExportDefault(expression) => self.variable(
                VariableKind::Const,
                kiln_hir::DEFAULT_EXPORT_LOCAL,
                Some(expression),
                statement.span,
            ),
            StatementKind::Unsupported { feature } => self.error(
                Some(statement.span),
                format!("unsupported statement: {feature}"),
            ),
        }
    }

    fn variable(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
    ) {
        self.variable_with(kind, name, initializer, span, false);
    }

    /// Bind `pattern` to the value on top of the stack, consuming it.
    fn bind_pattern(&mut self, kind: VariableKind, pattern: &Pattern, span: Span) {
        self.destructure(PatternLeaf::Declare(kind), pattern, span);
    }

    /// Assign `pattern` from the value on top of the stack, consuming it.
    ///
    /// Every rule about how a pattern takes a value apart -- defaults applying only to
    /// `undefined`, a hole skipping an index, a rest copying what the named keys did not take --
    /// is the same whether the names are being declared or assigned. Only the leaf differs.
    fn assign_pattern(&mut self, pattern: &Pattern, span: Span) {
        self.destructure(PatternLeaf::Assign, pattern, span);
    }

    /// Take the value on top of the stack apart, consuming it.
    ///
    /// A leaf that stores through a property reference expects the receiver -- and the key, for a
    /// computed one -- to be sitting under that value already: [`Compiler::destructure_element`]
    /// puts them there before the value exists, because that is the order the language evaluates
    /// them in. Only that method reaches such a leaf, which is why nothing here re-derives them.
    /// `RequireObjectCoercible` on the value an object pattern is about to take apart.
    ///
    /// Part of the pattern rather than a side effect of reading it: `var {} = null` throws even
    /// though the pattern names nothing to read. A pattern *with* properties only appeared to
    /// obey the rule, because reading its first key off `null` threw for an unrelated reason --
    /// which left `var {} = null`, `({} = null)` and `var { ...rest } = null` accepting a value
    /// no object pattern may take.
    fn require_object_coercible(&mut self) {
        self.code.push(Instruction::Dup);
        let coercible = self.emit_jump(Instruction::JumpIfNotNullish(0));
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "Cannot destructure 'null' or 'undefined'".to_owned(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
        self.patch_jump(coercible);
    }

    fn destructure(&mut self, leaf: PatternLeaf, pattern: &Pattern, span: Span) {
        match pattern {
            Pattern::Identifier(name) => {
                if name.is_empty() {
                    // The frontend already reported why; binding an impossible name here would
                    // add a second, more confusing diagnostic.
                    self.code.push(Instruction::Pop);
                    return;
                }
                match leaf {
                    PatternLeaf::Declare(kind) => self.declare_bound(kind, name, span),
                    PatternLeaf::Assign => self.store_existing(name, span),
                }
            }
            Pattern::Member { property, .. } => {
                let name = self.constant_index(Constant::String(property.clone()));
                self.store_through_reference(leaf, self.set_property_instruction(name), span);
            }
            Pattern::Computed { .. } => {
                self.store_through_reference(leaf, self.set_computed_instruction(), span);
            }
            Pattern::PrivateMember { name, .. } => {
                let brand = self.private_brand(name);
                let name = self.constant_index(Constant::String(self.private_key(name)));
                // The store is the first thing `store_through_reference` emits, so the brand
                // pushed here lands directly beneath it.
                self.push_private_brand(&brand);
                self.store_through_reference(leaf, Instruction::SetPrivate(name), span);
            }
            Pattern::WithDefault { inner, default } => {
                // A default applies only to `undefined`, not to every falsy value and not to
                // `null` -- which is why this is a nullish-style test against undefined alone
                // rather than a truthiness test.
                self.code.push(Instruction::Dup);
                self.push_constant(Constant::Undefined);
                self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
                let skip = self.emit_jump(Instruction::JumpIfFalse(0));
                self.code.push(Instruction::Pop);
                self.expression(default);
                self.patch_jump(skip);
                self.destructure(leaf, inner, span);
            }
            Pattern::Object { properties, rest } => {
                self.require_object_coercible();
                let spilled = rest.is_some()
                    || properties.iter().any(|property| {
                        stores_through_reference(&property.value)
                            || matches!(property.key, PatternKey::Computed(_))
                    });
                let source = self.pattern_source(spilled, span);

                let mut taken = Vec::with_capacity(properties.len());
                for property in properties {
                    // A computed key is evaluated where the source wrote it: before the target's
                    // reference and before the read, and exactly once even when a rest below
                    // needs the key again.
                    let key = match &property.key {
                        PatternKey::Static(name) => {
                            SourceKey::Static(self.constant_index(Constant::String(name.clone())))
                        }
                        PatternKey::Computed(expression) => {
                            self.expression(expression);
                            let held = format!("%key{}", self.hidden_bindings);
                            self.hidden_bindings += 1;
                            self.declare_hidden(&held, span);
                            SourceKey::Computed(held)
                        }
                    };
                    self.destructure_element(leaf, &property.value, span, |compiler| {
                        compiler.load_pattern_source(&source);
                        compiler.read_key(&key);
                    });
                    taken.push(key);
                }

                // Copy everything, then remove what the named properties already took. The other
                // direction -- copy all but a set of names -- would need the exclusion list at
                // runtime; this needs only instructions that already earn their keep.
                if let Some(rest) = rest {
                    self.destructure_element(leaf, rest, span, |compiler| {
                        compiler.code.push(Instruction::NewObject { count: 0 });
                        compiler.load_pattern_source(&source);
                        compiler.code.push(Instruction::SpreadObject);
                        for key in &taken {
                            compiler.code.push(Instruction::Dup);
                            match key {
                                SourceKey::Static(index) => {
                                    compiler
                                        .code
                                        .push(compiler.delete_property_instruction(*index));
                                }
                                SourceKey::Computed(held) => {
                                    compiler.load_hidden(held);
                                    compiler.code.push(compiler.delete_computed_instruction());
                                }
                            }
                            compiler.code.push(Instruction::Pop);
                        }
                    });
                }
                self.finish_pattern_source(&source);
            }
            Pattern::Array { elements, rest } => {
                self.destructure_array(leaf, elements, rest.as_deref(), span);
            }
        }
    }

    /// Take an array pattern apart by stepping its iterator, one element at a time.
    ///
    /// The shorter lowering -- materialise the iterable into an array and read it by index --
    /// gets three separate things wrong, and each of them is something a program can observe:
    ///
    /// * `const [a] = naturals()` must take exactly one step. Draining first never returns.
    /// * An iterator the pattern did not exhaust must be *closed*, by calling its `return`. A
    ///   generator on the other side of one runs its `finally` blocks because of that call.
    /// * How many times `next` ran is visible to whoever wrote the iterator, so `[] = iter` --
    ///   which takes nothing -- must not step it at all.
    ///
    /// So the protocol is walked here, in the same shape `for-of` walks it and for the same
    /// reason: no single instruction can stop part-way through an iterable.
    ///
    /// `done` tracks the specification's `iteratorRecord.[[Done]]`, and the flag is what decides
    /// whether the close happens. It is set *before* each `next` and cleared only once a value
    /// is in hand, because an iterator that threw is finished rather than merely unfinished --
    /// closing one that has already failed would call `return` on it, which the language does
    /// not do.
    fn destructure_array(
        &mut self,
        leaf: PatternLeaf,
        elements: &[Option<Pattern>],
        rest: Option<&Pattern>,
        span: Span,
    ) {
        let index = self.hidden_bindings;
        self.hidden_bindings += 1;
        let iterator = format!("%iterated{index}");
        let done = format!("%drained{index}");
        let step = format!("%stepped{index}");
        let next_method = format!("%nextof{index}");

        // `value[Symbol.iterator]()`, which is what makes `const [a] = new Set([1])` bind 1: a
        // pattern reads what the value yields, not what its indices hold.
        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        self.code.push(Instruction::CallMethod {
            name: symbol,
            argc: 0,
        });
        self.declare_hidden(&iterator, span);
        // The rest of the iterator record. `GetIteratorFromMethod` reads `next` once, before any
        // element is taken, and every step calls what it captured -- which is why `const [] = it`
        // reads `next` even though it steps nothing, and why a three-element pattern reads it
        // once rather than three times.
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.load_hidden(&iterator);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_method, span);
        self.push_constant(Constant::Boolean(false));
        self.declare_hidden(&done, span);
        // The slot the step results land in, bound once here so each element can store into it.
        self.push_constant(Constant::Undefined);
        self.declare_hidden(&step, span);

        // Everything from here to the close is guarded: an exception out of a default, a nested
        // pattern or a target's own evaluation has to close the iterator on its way past.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        for element in elements {
            match element {
                // A hole steps the iterator and reads nothing -- not even `value`, which is why
                // it is not the same as binding a name no one looks at.
                None => self.emit_iterator_step(&iterator, &next_method, &done, &step, false),
                Some(element) => {
                    self.destructure_element(leaf, element, span, |compiler| {
                        compiler.emit_iterator_step(&iterator, &next_method, &done, &step, true);
                    });
                }
            }
        }
        if let Some(rest) = rest {
            self.destructure_element(leaf, rest, span, |compiler| {
                compiler.emit_iterator_drain(&iterator, &next_method, &done, &step, span);
            });
        }

        self.code.push(Instruction::PopHandler);
        // A pattern that consumed every value has an iterator that closed itself; calling
        // `return` on an exhausted iterator is a step the language does not take.
        self.load_hidden(&done);
        let closed = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_close_checked(&iterator, span);
        self.patch_jump(closed);
        let finished = self.emit_jump(Instruction::Jump(0));

        // The landing pad. Entering it has already popped the handler and put the thrown value
        // on the stack. The close here is the forgiving flavour: an exception already on its way
        // out is not replaced by whatever the cleanup thought of the iterator.
        self.patch_jump(handler_jump);
        self.load_hidden(&done);
        let raise = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_close_quietly(&iterator, span);
        self.patch_jump(raise);
        self.code.push(Instruction::Throw);

        self.patch_jump(finished);
    }

    /// One `IteratorStep`, leaving the value it produced when `want_value` is set.
    ///
    /// An exhausted iterator yields `undefined` rather than stopping the walk: `var [a, b] = [1]`
    /// binds `b` to undefined, and a default on it applies.
    fn emit_iterator_step(
        &mut self,
        iterator: &str,
        next_method: &str,
        done: &str,
        step: &str,
        want_value: bool,
    ) {
        self.load_hidden(done);
        let exhausted = self.emit_jump(Instruction::JumpIfTruthy(0));
        // Marked done *before* the call, and cleared only after a value is in hand: a `next` or
        // a `value` getter that throws leaves the iterator failed, and a failed one is never
        // closed.
        self.push_constant(Constant::Boolean(true));
        self.store_hidden(done);
        self.load_hidden(iterator);
        self.load_hidden(next_method);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        self.guard_iterator_result();
        self.store_hidden(step);
        self.load_hidden(step);
        let finished_key = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(finished_key));
        let finished = self.emit_jump(Instruction::JumpIfTruthy(0));
        if want_value {
            self.load_hidden(step);
            let value_key = self.constant_index(Constant::String("value".to_owned()));
            self.code.push(Instruction::GetProperty(value_key));
        }
        self.push_constant(Constant::Boolean(false));
        self.store_hidden(done);
        let ready = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(exhausted);
        self.patch_jump(finished);
        if want_value {
            self.push_constant(Constant::Undefined);
        }
        self.patch_jump(ready);
    }

    /// `...rest`: everything the iterator has left, as a fresh array.
    ///
    /// Written with an index and `SetComputed` rather than with `push`, because
    /// `Array.prototype.push` is a property a program can replace and the language defines this
    /// as a plain data property on a fresh array.
    fn emit_iterator_drain(
        &mut self,
        iterator: &str,
        next_method: &str,
        done: &str,
        step: &str,
        span: Span,
    ) {
        let index = self.hidden_bindings;
        self.hidden_bindings += 1;
        let collected = format!("%collected{index}");
        let count = format!("%counted{index}");
        self.code.push(Instruction::NewArray { count: 0 });
        self.declare_hidden(&collected, span);
        self.push_constant(Constant::Number(0.0f64.to_bits()));
        self.declare_hidden(&count, span);

        let start = self.code.len();
        self.load_hidden(done);
        let drained = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.push_constant(Constant::Boolean(true));
        self.store_hidden(done);
        self.load_hidden(iterator);
        self.load_hidden(next_method);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        self.guard_iterator_result();
        self.store_hidden(step);
        self.load_hidden(step);
        let finished_key = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(finished_key));
        let ended = self.emit_jump(Instruction::JumpIfTruthy(0));

        self.load_hidden(&collected);
        self.load_hidden(&count);
        self.load_hidden(step);
        let value_key = self.constant_index(Constant::String("value".to_owned()));
        self.code.push(Instruction::GetProperty(value_key));
        // The value survived, so the iterator is unfinished again.
        self.push_constant(Constant::Boolean(false));
        self.store_hidden(done);
        self.code.push(self.set_computed_instruction());
        self.code.push(Instruction::Pop);
        self.load_hidden(&count);
        self.push_constant(Constant::Number(1.0f64.to_bits()));
        self.code.push(Instruction::Binary(BinaryOp::Add));
        self.store_hidden(&count);
        self.code.push(Instruction::Jump(start as u32));

        self.patch_jump(drained);
        self.patch_jump(ended);
        self.load_hidden(&collected);
    }

    /// `IteratorClose` on a normal completion, which is the strict flavour of it.
    ///
    /// A `return` that throws propagates, and one that answers a non-object is a `TypeError`:
    /// nothing is on its way out yet, so this failure is the program's outcome. The pad that
    /// catches an exception uses [`Compiler::emit_iterator_close`] instead, which discards both.
    fn emit_iterator_close_checked(&mut self, binding: &str, span: Span) {
        let absent = self.emit_iterator_return(binding, span);
        self.require_object("Iterator result of `return` is not an object");
        self.code.push(Instruction::Pop);
        self.patch_jump(absent);
    }

    /// `IteratorClose` on an abrupt completion, which is the forgiving flavour of it.
    ///
    /// The iterator is given its chance to clean up and anything it has to say about that is
    /// dropped -- its result unread, and an exception out of its `return` swallowed. A value is
    /// already travelling outwards, and the language keeps the one the program threw rather than
    /// replacing it with whatever the cleanup thought.
    fn emit_iterator_close_quietly(&mut self, binding: &str, span: Span) {
        let guard = self.emit_jump(Instruction::PushHandler(0));
        let absent = self.emit_iterator_return(binding, span);
        self.code.push(Instruction::Pop);
        self.patch_jump(absent);
        self.code.push(Instruction::PopHandler);
        let closed = self.emit_jump(Instruction::Jump(0));
        // Entering here truncated the stack back to where the guard went up, so the only thing
        // above the exception being carried out is the one the `return` raised.
        self.patch_jump(guard);
        self.code.push(Instruction::Pop);
        self.patch_jump(closed);
    }

    /// Call the iterator's `return`, leaving its result on the stack.
    ///
    /// Returns the jump taken when there is no `return` to call, which the caller patches to
    /// wherever it lands after dealing with the result: an absent `return` leaves nothing.
    ///
    /// Applied rather than called by name, because the language reads `return` once and a second
    /// read would run an accessor twice. The empty argument array is also what `return` sees as
    /// its own `arguments`, whose length one test asserts.
    fn emit_iterator_return(&mut self, binding: &str, span: Span) -> usize {
        let name = self.constant_index(Constant::String("return".to_owned()));
        let held = format!("%closing{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.load_hidden(binding);
        self.code.push(Instruction::GetProperty(name));
        self.declare_hidden(&held, span);
        self.load_hidden(&held);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        let absent = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(present);
        self.load_hidden(binding);
        self.load_hidden(&held);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        absent
    }

    /// Throw a `TypeError` unless the value on top of the stack is an object, leaving it either
    /// way. `typeof` rather than a test of its own, because the two answers that mean "object"
    /// are exactly `"object"` -- excluding `null`, which shares the answer -- and `"function"`.
    fn require_object(&mut self, message: &str) {
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let nullable = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));
        let refused = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(nullable);
        self.code.push(Instruction::Dup);
        let object = self.emit_jump(Instruction::JumpIfNotNullish(0));

        self.patch_jump(refused);
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(message.to_owned()));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);

        self.patch_jump(callable);
        self.patch_jump(object);
    }

    /// Take one element or property apart, given how to push the value it receives.
    ///
    /// The value is emitted by a callback rather than passed in already on the stack, because a
    /// target that stores through a reference has to be evaluated *before* it: `({ a: o.p } = s)`
    /// evaluates `o` and only then runs the getter for `a`, and which of the two throws first is
    /// observable. A nested pattern is not a reference, so `[{ a: o.p }] = xs` evaluates `o`
    /// while taking the inner object apart rather than before reading the element.
    fn destructure_element(
        &mut self,
        leaf: PatternLeaf,
        target: &Pattern,
        span: Span,
        value: impl FnOnce(&mut Self),
    ) {
        // In declaration position a reference target is refused rather than evaluated, so
        // nothing is pushed here that the leaf below would not know to pop.
        if matches!(leaf, PatternLeaf::Assign) {
            self.reference_prologue(target);
        }
        value(self);
        self.destructure(leaf, target, span);
    }

    /// Evaluate what a target stores through, looking past any default it carries.
    fn reference_prologue(&mut self, target: &Pattern) {
        match target {
            Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
                self.expression(object);
            }
            Pattern::Computed { object, property } => {
                self.expression(object);
                self.expression(property);
            }
            // `[o.p = 1] = xs` evaluates `o` before the element, not after the default decides
            // whether one is needed: the default is part of the value, not part of the target.
            Pattern::WithDefault { inner, .. } => self.reference_prologue(inner),
            _ => {}
        }
    }

    /// Finish a leaf that stores through a reference, with the value on top of the stack and the
    /// reference under it.
    fn store_through_reference(&mut self, leaf: PatternLeaf, store: Instruction, span: Span) {
        match leaf {
            PatternLeaf::Assign => {
                self.code.push(store);
                // The store leaves the value, which is what an assignment expression evaluates
                // to and what a pattern walk has no further use for.
                self.code.push(Instruction::Pop);
            }
            // `let [o.p] = xs` is a syntax error: a declaration introduces names, and a property
            // reference is not one. Nothing was pushed for the reference, so only the value goes.
            PatternLeaf::Declare(_) => {
                self.error(
                    Some(span),
                    "a declaration cannot bind through a property reference",
                );
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Move the value being taken apart off the stack when the walk needs the room.
    ///
    /// The temporary deliberately goes in the scope that is current, rather than in one pushed
    /// around the walk: the names the pattern binds are declared as it runs, and a scope of the
    /// walk's own would swallow them when it was popped.
    fn pattern_source(&mut self, spilled: bool, span: Span) -> PatternSource {
        if !spilled {
            return PatternSource::Stack;
        }
        let name = format!("%source{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.declare_hidden(&name, span);
        PatternSource::Hidden(name)
    }

    fn load_pattern_source(&mut self, source: &PatternSource) {
        match source {
            PatternSource::Stack => self.code.push(Instruction::Dup),
            PatternSource::Hidden(name) => self.load_hidden(name),
        }
    }

    /// Read one property of the source, which is already on top of the stack.
    fn read_key(&mut self, key: &SourceKey) {
        match key {
            SourceKey::Static(index) => self.code.push(Instruction::GetProperty(*index)),
            SourceKey::Computed(held) => {
                self.load_hidden(held);
                self.code.push(Instruction::GetComputed);
            }
        }
    }

    /// Drop the source now the walk is done with it. A spilled one is already off the stack.
    fn finish_pattern_source(&mut self, source: &PatternSource) {
        if matches!(source, PatternSource::Stack) {
            self.code.push(Instruction::Pop);
        }
    }

    /// Declare a binding.
    ///
    /// `bound_from_stack` is for bindings the compiler introduces itself, where the value is
    /// already on the stack and the caller stores it: a `for-of` loop variable is `const` and
    /// initialised, but not by an expression this function can see. Without the distinction the
    /// const check would reject a binding that is in fact initialised.
    fn variable_with(
        &mut self,
        kind: VariableKind,
        name: &str,
        initializer: Option<&Expression>,
        span: Span,
        bound_from_stack: bool,
    ) {
        if name.is_empty() {
            self.error(Some(span), "destructuring bindings are not supported yet");
            return;
        }
        // `var x = e` inside a `with` initialises through the object environment, because the
        // declaration and the initialisation are two different things: the *declaration* hoists to
        // the function or script scope, and the *initialisation* is an ordinary assignment, so it
        // resolves `x` against the with object first and writes there when that object has the
        // name. The hoisted binding is then left undefined -- `with (o) { var value = "v" }` makes
        // `o.value` "v" and leaves `value` undefined. Storing into the slot got both backwards.
        //
        // Only a plain `var`: a `let` or `const` inside a `with` block declares in the block's own
        // scope, which the object environment does not shadow.
        if kind == VariableKind::Var
            && !bound_from_stack
            && let Some(initializer) = initializer
        {
            let scopes = self.with_scopes_for(name);
            if !scopes.is_empty() {
                self.with_assignment(&scopes, AssignmentOperator::Assign, name, initializer, span);
                // An assignment leaves its value; a declaration is a statement and wants none.
                self.code.push(Instruction::Pop);
                return;
            }
        }
        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        // A `var` at the top of global code is a property of the global object, and declaration
        // instantiation already created it. All that is left is the initializer.
        // `global_var_scope` and not merely the name being in `global_bindings`: a function
        // written in a script whose top level declares `var x` still has its own `var x`, and
        // that one is a slot of the call's.
        if self.global_var_scope
            && kind == VariableKind::Var
            && scope_index == 0
            && self.global_bindings.contains(name)
        {
            if bound_from_stack {
                // The value is on the stack and `declare_bound` emits the store, because only it
                // knows which instruction the binding needs.
                return;
            }
            // `var x;` with no initializer keeps whatever the binding already holds -- which is
            // the same rule a slot-backed `var` follows, and the reason a second `var x;` does
            // not reset it to `undefined`.
            if let Some(initializer) = initializer {
                self.expression(initializer);
                self.store_global_object_binding(name);
            }
            return;
        }
        if let Some(existing) = self.scopes[scope_index].get(name).copied() {
            // A `var` re-declaration is an assignment, and the hoisting pass has already created
            // the slot for every declaration in the block -- so reaching an existing binding is
            // the normal path now, and a *second* declaration of the same `let` is the error.
            // Two `var`s of one name are an assignment, not a redeclaration. Every other pairing
            // is an error -- including `var` after `let`, which is why the kinds of *both* are
            // consulted rather than only the one being declared now.
            let redeclaration = existing.initialized
                && !(kind == VariableKind::Var && existing.kind == VariableKind::Var);
            if redeclaration {
                self.error(
                    Some(span),
                    format!("redeclaration of local `{name}` is not supported"),
                );
                return;
            }
            if kind == VariableKind::Const && initializer.is_none() && !bound_from_stack {
                self.error(
                    Some(span),
                    format!("const local `{name}` requires an initializer"),
                );
            }
            if bound_from_stack {
                self.initialize(name);
                return;
            }
            // The initializer is compiled *before* the name leaves its dead zone, because it is
            // inside that zone: `let x = x` is a run-time `ReferenceError`, not a read of an
            // undefined slot and not a refusal of the source.
            if let Some(initializer) = initializer {
                self.expression(initializer);
            } else if kind != VariableKind::Var {
                // A `let` with no initializer is undefined; a `var` keeps what it already had,
                // which is what makes `var x = 1; var x;` leave 1 in place.
                self.push_constant(Constant::Undefined);
            } else {
                self.initialize(name);
                return;
            }
            self.initialize(name);
            self.code.push(if existing.celled {
                Instruction::StoreCell(existing.slot)
            } else {
                Instruction::StoreLocal(existing.slot)
            });
            return;
        }
        if kind == VariableKind::Const && initializer.is_none() && !bound_from_stack {
            self.error(
                Some(span),
                format!("const local `{name}` requires an initializer"),
            );
        }
        let slot = self.local_count;
        self.local_count += 1;
        // A captured binding is a cell at every scope, including module scope. `LoadGlobal`
        // reaches the currently executing module frame, which is correct only while a callable
        // stays in its declaring module. Exported functions and classes travel; their cells travel
        // with them. `predeclare` applies the same rule to hoisted bindings, and keeping this
        // non-hoisted path weaker lets hidden superclass bindings index the importer by slot.
        let celled = self.cell_everything || self.captured.contains(name);
        // A `let` or `const` whose initializer has not run yet is in its dead zone, and the
        // initializer is part of that zone. A `var` never is, and a binding the caller is about
        // to store into from the stack has no initializer of its own to wait for.
        let in_dead_zone = kind != VariableKind::Var && !bound_from_stack;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized: !in_dead_zone,
                celled,
            },
        );
        if celled {
            self.code.push(Instruction::NewCell(slot));
            // A captured binding can be read from a function called before the declaration runs,
            // where the compiler cannot see the read. The marker is what makes that a
            // `ReferenceError` rather than an undefined the program never wrote. Uncaptured
            // bindings use a scratch marker below when their initializer reads them directly.
            if in_dead_zone {
                self.code.push(Instruction::MarkUninitialized(slot));
            }
        }
        if bound_from_stack {
            // The value is already on the stack; the caller emits the store, because it may need
            // to know which instruction was used.
            return;
        }
        if let Some(initializer) = initializer {
            self.expression(initializer);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.initialize(name);
        self.code.push(if celled {
            Instruction::StoreCell(slot)
        } else {
            Instruction::StoreLocal(slot)
        });
    }

    /// Compile a run of statements sharing one scope, hoisting what the language hoists.
    ///
    /// `f(); function f() {}` is ordinary JavaScript, so a function declaration's value has to
    /// exist before the line that writes it. Emitting those first is not enough on its own: a
    /// hoisted body would then compile before the `const`s below it are in scope, and a closure
    /// legitimately sees those. So the block's *names* are all created first and only the
    /// function declarations' *values* are emitted early -- which is what the language does, and
    /// is where its temporal dead zone comes from.
    fn statements(&mut self, list: &[Statement]) {
        let resources = resource_declarations(list);
        if resources.is_empty() {
            self.plain_statements(list);
            return;
        }
        self.resource_scope(list, &resources);
    }

    /// Compile a statement list whose `using` declarations have to be disposed of when control
    /// leaves it, whichever way it leaves.
    ///
    /// The shape is `try`/`finally` with no `catch`, and for the same reason: a disposal has to
    /// run on every way out, and re-emitting it at each exit is what keeps `break`, `continue`
    /// and `return` ordinary jumps. It goes on the same cleanup stack a `finally` uses, so a
    /// non-local exit crossing both runs them in the right order without either knowing about
    /// the other.
    fn resource_scope(&mut self, list: &[Statement], resources: &[bool]) {
        let span = list
            .first()
            .map_or(Span::new(0, 0), |statement| statement.span);
        self.with_resource_scope(resources, span, |compiler| compiler.plain_statements(list));
    }

    /// Compile `body` with `resources` disposed of on every way out of it.
    fn with_resource_scope(
        &mut self,
        resources: &[bool],
        span: Span,
        body: impl FnOnce(&mut Self),
    ) {
        let mut slots = Vec::with_capacity(resources.len());
        for awaiting in resources {
            let index = self.hidden_bindings;
            self.hidden_bindings += 1;
            let slot = ResourceSlot {
                value: format!("%resource{index}"),
                method: format!("%dispose{index}"),
                awaiting: *awaiting,
            };
            // Cleared on entry rather than only written on use, because a loop body is one scope
            // entered many times: without this, a pass that never reached a declaration would
            // dispose of what the previous pass left in the slot.
            self.push_constant(Constant::Undefined);
            self.declare_hidden(&slot.value, span);
            self.push_constant(Constant::Undefined);
            self.declare_hidden(&slot.method, span);
            slots.push(slot);
        }
        let cleanup = Cleanup::DisposeResources(slots.clone());
        self.resource_scopes.push(ResourceScope { slots, next: 0 });
        self.finalizers.push(cleanup.clone());

        let handler_jump = self.emit_jump(Instruction::PushHandler(0));
        self.handler_depth += 1;
        body(self);
        self.code.push(Instruction::PopHandler);
        self.handler_depth -= 1;

        self.finalizers.pop();
        self.emit_finalizer(&cleanup);
        let done = self.emit_jump(Instruction::Jump(0));

        // The exception is still on the stack for `Throw` to take; the disposals below it push
        // and pop in balance, so it is still on top when they are done.
        self.patch_jump(handler_jump);
        self.emit_finalizer(&cleanup);
        self.code.push(Instruction::Throw);
        self.patch_jump(done);
        self.resource_scopes
            .pop()
            .expect("this scope pushed its own resource state");
    }

    fn plain_statements(&mut self, list: &[Statement]) {
        for statement in list {
            self.hoist_names(statement);
        }
        for statement in list {
            if let StatementKind::Function(function) = &statement.kind {
                self.emit_function_declaration(function, statement.span);
            }
        }
        for statement in list {
            // Emitted above; running it again would rebuild the closure and overwrite the
            // binding with an identical one.
            if matches!(statement.kind, StatementKind::Function(_)) {
                continue;
            }
            self.statement(statement);
        }
    }

    /// Read `name` from the bindings around it, ignoring any `with` object.
    ///
    /// The whole of Kiln's ordinary identifier resolution: a slot, a cell, an upvalue, a module
    /// slot, or -- when nothing bound it -- a built-in or a `ReferenceError` where the read is.
    fn load_lexically(&mut self, name: &str) {
        match self.resolve_or_capture(name) {
            // Reading a binding whose declaration has not run yet is the temporal dead zone.
            // Keep the read in the instruction stream: if control never reaches it, compilation
            // succeeds, and if it does, the marker produces the language's `ReferenceError`.
            // A nested function reading the same name resolves as an upvalue and checks the
            // shared cell when it runs.
            Resolution::Local(local) if !local.initialized && local.celled && self.eval_program => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            // Any read of a binding whose declaration has not run yet: `x; let x = 1`,
            // `let x = x`, or a parameter read from another parameter's default in
            // `function f(x = y, y) {}`. The check has to be a *run-time* `ReferenceError`,
            // because the source is well formed and only reaching the read fails -- exactly what
            // `assert.throws(ReferenceError, () => f())` asks for. A cell that is uninitialised by
            // construction is that check, spelled with instructions that already exist rather
            // than with an opcode of its own; a scratch one rather than the binding's own,
            // because a plain local has no cell at this point in the code and an uncelled slot is
            // not one to load from.
            Resolution::Local(local) if !local.initialized => {
                let _ = local;
                let scratch = self.local_count;
                self.local_count += 1;
                self.code.push(Instruction::NewCell(scratch));
                self.code.push(Instruction::MarkUninitialized(scratch));
                self.code.push(Instruction::LoadCell(scratch));
            }
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::GlobalObject => self.load_global_object_binding(name),
            Resolution::Unresolved if name == "this" => self.code.push(Instruction::LoadThis),
            // `arguments` is not a declared binding, so it only resolves when nothing else
            // bound the name -- a parameter or local called `arguments` shadows it, as it
            // does in JavaScript.
            Resolution::Unresolved if name == "arguments" && self.compiling_function() => {
                self.code.push(Instruction::LoadArguments)
            }
            Resolution::Unresolved if name == "undefined" => {
                self.push_constant(Constant::Undefined)
            }
            // A built-in is only reachable when nothing else bound the name, so a program
            // that declares its own `Error` shadows it exactly as it would any other global.
            Resolution::Unresolved => match Builtin::from_name(name) {
                Some(builtin) => self.code.push(Instruction::LoadBuiltin(builtin)),
                // A `ReferenceError` where the read happens, which is what JavaScript does
                // and what lets a module compile when a branch it never takes mentions a
                // global this realm does not provide. See ADR 0017.
                None => {
                    let name = self.constant_index(Constant::String(name.to_owned()));
                    self.code.push(Instruction::ThrowReference(name));
                }
            },
        }
    }

    /// `with (object) body`: an object environment record around the body's scope.
    ///
    /// The object goes into a hidden binding rather than staying on the stack, because the body
    /// may create a function that outlives the statement -- and that function's names still
    /// resolve against the object when it is called. A binding is the only thing a closure can
    /// capture, so the object becomes one, and it is boxed like any other captured binding.
    fn with_statement(&mut self, object: &Expression, body: &Statement, span: Span) {
        // Each level costs one lookup chain at every name inside it, and an assignment repeats
        // its value expression once per level. Four is far past anything written on purpose, and
        // refusing beyond it is a limit that says so rather than emitting code nobody can read.
        if self.with_scopes.len() >= 4 {
            self.error(
                Some(span),
                "unsupported statement: `with` nested more than four deep",
            );
            return;
        }
        let name = format!("%with{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());
        self.captured.insert(name.clone());
        self.expression(object);
        self.code.push(Instruction::ToObject);
        self.declare_bound(VariableKind::Let, &name, span);
        self.with_scopes.push(WithScope {
            name,
            functions: self.enclosing.len(),
            scopes: self.scopes.len(),
        });
        self.statement(body);
        self.with_scopes.pop().expect("just pushed");
        self.scopes.pop().expect("with scope was just pushed");
    }

    /// The `with` objects `name` has to be looked for on, innermost first.
    ///
    /// Empty for every name in a program with no `with` in it, and for a name whose binding is
    /// written *inside* the body: `with (o) { let x = 1; x }` is the `let`, whatever `o` holds,
    /// because the block's scope is nested within the object environment record.
    fn with_scopes_for(&self, name: &str) -> Vec<WithScope> {
        if self.with_scopes.is_empty() {
            return Vec::new();
        }
        // `this` is not a binding an environment record answers for, and a compiler-introduced
        // name is not one any object can have: neither is ever looked for on a `with` object.
        if name == "this" || name.starts_with('%') || name.starts_with('*') {
            return Vec::new();
        }
        let bound = self
            .scopes
            .iter()
            .rposition(|scope| scope.contains_key(name));
        let functions = self.enclosing.len();
        self.with_scopes
            .iter()
            .rev()
            .filter(|with| {
                if with.functions == functions {
                    bound.is_none_or(|at| at < with.scopes)
                } else {
                    // The `with` is in an enclosing function, so every binding this function
                    // declares -- a parameter, a local -- is written inside it and shadows it.
                    bound.is_none()
                }
            })
            .cloned()
            .collect()
    }

    /// Push the object of one active `with`.
    fn load_with_object(&mut self, with: &WithScope) {
        match self.resolve_or_capture(&with.name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            // A hidden binding, never a name a program could have declared on the object.
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, "a `with` object went out of scope");
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// Read `name`, looking on each active `with` object before the bindings around it.
    ///
    /// `fallback` emits the read for the case no object has it -- the ordinary lexical one, or
    /// the `"undefined"` that a `typeof` of an unbound name produces.
    /// `delete name` where `name` is an unqualified identifier, which sloppy code alone can
    /// write.
    ///
    /// A `with` object is asked first, exactly as a *read* of the same name asks it: the name may
    /// be absent from every binding around the statement and still be a property of the object,
    /// and it is that property `delete` removes.
    fn delete_binding(&mut self, name: &str) {
        let scopes = self.with_scopes_for(name);
        if scopes.is_empty() {
            self.delete_binding_lexically(name);
            return;
        }
        let key = self.constant_index(Constant::String(name.to_owned()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in &scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::DeleteProperty(key));
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.delete_binding_lexically(name);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    fn delete_binding_lexically(&mut self, name: &str) {
        match self.resolve(name) {
            // Anything a scope declared is undeletable -- `var`, `let`, a parameter, a
            // function declaration, an import -- so the answer is known here and nothing
            // needs to run.
            Resolution::Local(_) | Resolution::Upvalue(_) | Resolution::Global(_) => {
                self.push_constant(Constant::Boolean(false));
            }
            // A global `var` is a property like any other: whether it can be deleted is the
            // property's own business, and a script's is non-configurable while an `eval`'s is
            // not. So the object is asked, exactly as it is for a name nothing declared.
            Resolution::GlobalObject => {
                let name = self.constant_index(Constant::String(name.to_owned()));
                self.code.push(Instruction::DeleteGlobalBinding(name));
            }
            // Nothing placed it, so the global object's environment record is the last one
            // an unqualified reference reaches and its `[[Delete]]` is the answer.  That is
            // also right for the names the compiler otherwise short-circuits -- `undefined`
            // and the intrinsics -- because the realm registers those as properties whose
            // attributes are exactly what decides `delete`.
            // `arguments` inside a function is a binding the call created, not a global: it is
            // undeletable like any other, and asking the global object would answer `true` for a
            // name that is right there.
            Resolution::Unresolved if name == "arguments" && self.compiling_function() => {
                self.push_constant(Constant::Boolean(false));
            }
            Resolution::Unresolved => {
                let name = self.constant_index(Constant::String(name.to_owned()));
                self.code.push(Instruction::DeleteGlobalBinding(name));
            }
        }
    }

    fn with_scoped_load(&mut self, name: &str, fallback: impl FnOnce(&mut Self)) {
        let scopes = self.with_scopes_for(name);
        if scopes.is_empty() {
            fallback(self);
            return;
        }
        let key = self.constant_index(Constant::String(name.to_owned()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in &scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::GetProperty(key));
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        fallback(self);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// Create the bindings one statement declares, without running its initializer.
    fn hoist_names(&mut self, statement: &Statement) {
        match &statement.kind {
            // A function declaration is usable immediately; a class is not, which is the whole
            // difference between the two for hoisting purposes.
            StatementKind::Function(function) => {
                if let Some(name) = &function.name {
                    // At the top of global code a function declaration is a *var*-scoped
                    // binding -- `CreateGlobalFunctionBinding` -- and so a property of the
                    // global object. Anywhere else it is block-scoped like a `let`.
                    let kind = if self.global_var_scope && self.scopes.len() == 1 {
                        VariableKind::Var
                    } else {
                        VariableKind::Let
                    };
                    self.predeclare(kind, name, true);
                }
            }
            StatementKind::Class(class) => {
                if let Some(name) = &class.name {
                    self.predeclare(VariableKind::Let, name, false);
                }
            }
            StatementKind::Variable { kind, declarations } => {
                for declaration in declarations {
                    let mut names = Vec::new();
                    pattern_names_in_order(&declaration.pattern, &mut names);
                    for name in names {
                        // `var` is initialised to undefined when the scope is entered; `let` and
                        // `const` are not, and reading one before its declaration is an error.
                        self.predeclare(*kind, &name, *kind == VariableKind::Var);
                    }
                }
            }
            _ => {}
        }
    }

    /// Predeclare every `var` a body contains, wherever inside it the declaration is written.
    ///
    /// `hoist_names` runs once per *block*, which is right for a lexical declaration and wrong
    /// for a `var`: a `var` belongs to the function, not to the block it appears in. Its slot
    /// already went to scope 0, but the instructions `predeclare` emits went wherever the block
    /// was compiled -- so a captured `var` written inside one arm of an `if` had its `NewCell`
    /// inside that arm, and the *other* arm's write found an unboxed slot. That is
    /// `local slot 460 does not hold a captured cell`, and `import "react-dom/server"` is
    /// exactly it -- `if (typeof performance === "object") { var getCurrentTime = ... } else {
    /// getCurrentTime = ... }`, with the closure that reads `getCurrentTime` being what makes
    /// the binding boxed and the else path fatal.
    ///
    /// The initialisation is the same story told quietly: a `var` reads as `undefined` from the
    /// top of its function, not from the top of the block it was written in.
    ///
    /// Walks into every statement that shares this variable environment and into none that
    /// starts a new one, which is the specification's `VarScopedDeclarations`: a nested function
    /// owns its own `var`s, and a class body is a function's worth of scopes away.
    fn hoist_var_names(&mut self, body: &[Statement]) {
        for statement in body {
            self.hoist_var_names_in(statement);
        }
    }

    fn hoist_var_names_in(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Variable {
                kind: VariableKind::Var,
                declarations,
            } => {
                for declaration in declarations {
                    let mut names = Vec::new();
                    pattern_names_in_order(&declaration.pattern, &mut names);
                    for name in names {
                        self.predeclare(VariableKind::Var, &name, true);
                    }
                }
            }
            StatementKind::Block(body) => self.hoist_var_names(body),
            StatementKind::If {
                consequent,
                alternate,
                ..
            } => {
                self.hoist_var_names_in(consequent);
                if let Some(alternate) = alternate {
                    self.hoist_var_names_in(alternate);
                }
            }
            StatementKind::While { body, .. }
            | StatementKind::DoWhile { body, .. }
            | StatementKind::Labelled { body, .. }
            | StatementKind::With { body, .. } => self.hoist_var_names_in(body),
            StatementKind::For { init, body, .. } => {
                if let Some(init) = init {
                    self.hoist_var_names_in(init);
                }
                self.hoist_var_names_in(body);
            }
            StatementKind::ForIn {
                kind, name, body, ..
            } => {
                if *kind == VariableKind::Var {
                    self.predeclare(VariableKind::Var, name, true);
                }
                self.hoist_var_names_in(body);
            }
            StatementKind::ForOf {
                kind,
                pattern,
                body,
                ..
            } => {
                if *kind == VariableKind::Var {
                    let mut names = Vec::new();
                    pattern_names_in_order(pattern, &mut names);
                    for name in names {
                        self.predeclare(VariableKind::Var, &name, true);
                    }
                }
                self.hoist_var_names_in(body);
            }
            StatementKind::Switch { cases, .. } => {
                for case in cases {
                    self.hoist_var_names(&case.body);
                }
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                self.hoist_var_names(block);
                if let Some(handler) = handler {
                    self.hoist_var_names(&handler.body);
                }
                self.hoist_var_names(finalizer);
            }
            // A function owns its own `var`s, and everything else declares nothing.
            _ => {}
        }
    }

    /// Give every block-level function declaration the var-scoped binding Annex B promises it.
    ///
    /// B.3.2.1 for a function body and B.3.3.2 for a script: in sloppy code a `FunctionDeclaration`
    /// directly inside a `Block`, a `CaseClause`, a `DefaultClause` or an `if` branch also
    /// declares a `var` of the same name in the enclosing variable environment, initialised to
    /// `undefined` and written when the declaration is evaluated. It is the reason
    /// `{ function f() {} } typeof f` is `"function"` in a browser and `"undefined"` under a
    /// reading of the main grammar alone, and bundlers still emit code that depends on it.
    ///
    /// A name is skipped when replacing the declaration with a `var` would be an early error --
    /// a `let`, `const`, `class` or `using` of the same name anywhere between the variable
    /// environment and the block, a destructured catch parameter -- and, in a function, when it
    /// is a formal parameter or `arguments`. `blocked` carries exactly those names down.
    fn hoist_annexb_function_names(&mut self, body: &[Statement], parameters: &HashSet<String>) {
        if self.strict {
            return;
        }
        let mut blocked: Vec<String> = parameters.iter().cloned().collect();
        let mut names = Vec::new();
        collect_annexb_function_names(body, false, &mut blocked, &mut names);
        for name in names {
            if self.annexb_var_functions.contains(&name) {
                continue;
            }
            // A `var` or a top-level function declaration of the same name already owns the
            // binding; Annex B only adds the write, never a second slot.
            self.predeclare(VariableKind::Var, &name, true);
            self.annexb_var_functions.insert(name);
        }
    }

    /// The var-scoped write Annex B adds to a block-level function declaration's evaluation.
    ///
    /// Emitted after the block-scoped binding has been given its value, and reading that binding
    /// rather than rebuilding the closure: the specification copies what the block holds, so the
    /// two bindings start as the same function object and diverge from there --
    /// `{ function f() {} f = 123; }` leaves the outer `f` a function.
    fn emit_annexb_var_write(&mut self, name: &str, block: Local) {
        if !self.annexb_var_functions.contains(name) {
            return;
        }
        let Some(target) = self.scopes[0].get(name).copied() else {
            // The var-scoped binding is a property of the global object, so the copy is a write
            // to it rather than to a slot.
            if self.global_var_scope && self.global_bindings.contains(name) {
                self.code.push(if block.celled {
                    Instruction::LoadCell(block.slot)
                } else {
                    Instruction::LoadLocal(block.slot)
                });
                self.store_global_object_binding(name);
            }
            return;
        };
        // The declaration is at the top level of the variable environment after all, so the
        // binding it just wrote *is* the var-scoped one.
        if target.slot == block.slot {
            return;
        }
        self.code.push(if block.celled {
            Instruction::LoadCell(block.slot)
        } else {
            Instruction::LoadLocal(block.slot)
        });
        self.code.push(if target.celled {
            Instruction::StoreCell(target.slot)
        } else {
            Instruction::StoreLocal(target.slot)
        });
    }

    /// Allocate a slot for a name the block declares, leaving its value for the declaration.
    ///
    /// Silent when the name is already there: a genuine redeclaration is reported by the
    /// declaration itself, where the span points at the second one rather than at the block.
    fn predeclare(&mut self, kind: VariableKind, name: &str, initialized: bool) {
        if name.is_empty() {
            return;
        }
        let scope_index = if kind == VariableKind::Var {
            0
        } else {
            self.scopes.len() - 1
        };
        // An outer binding is the *caller's*, borrowed into scope 0 so a direct eval can read and
        // write it. A declaration in the eval'd source either takes it over or shadows it, and
        // which of the two is the whole difference strictness makes to an eval: a sloppy `var` or
        // function declaration belongs to the caller's variable environment and so becomes that
        // binding, while a `let`, a `const`, a class, and every declaration in strict code belong
        // to the eval's own scope and get a slot of their own.
        // `EvalDeclarationInstantiation` refuses a `var` that names a binding in any record
        // between this eval's lexical environment and its variable one. A block's `let` arrives
        // as an outer binding and is caught by the redeclaration rule below; a parameter
        // environment does not, because a function's parameters and its body share one scope
        // here -- so its names travelled from the call site instead.
        if kind == VariableKind::Var
            && scope_index == 0
            && self.blocked_var_names.iter().any(|blocked| blocked == name)
        {
            self.error(
                None,
                format!(
                    "a direct `eval` may not declare `var {name}` where the enclosing parameter                      list already binds that name"
                ),
            );
            return;
        }
        if scope_index == 0 && self.outer_names.remove(name) {
            if initialized && !self.strict {
                // `var x;` over an existing binding keeps its value -- which is why this returns
                // rather than falling through to the `undefined` store below.
                return;
            }
            self.scopes[0].remove(name);
        }
        if self.scopes[scope_index].contains_key(name) {
            return;
        }
        // Global code's variable environment is the global object, so a `var` or a function
        // declaration at the top of it is a property rather than a slot. Lexical declarations
        // are not: `let` and `const` at the top of a script go in the global *declarative*
        // record, where `globalThis` cannot see them, and they keep their slots.
        if self.global_var_scope && scope_index == 0 && kind == VariableKind::Var {
            if self.global_bindings.insert(name.to_owned()) {
                let index = self.constant_index(Constant::String(name.to_owned()));
                self.code.push(Instruction::DeclareGlobalBinding {
                    name: index,
                    configurable: self.global_bindings_configurable,
                });
            }
            return;
        }
        let slot = self.local_count;
        self.local_count += 1;
        // Module scope boxes a captured binding exactly like any other scope. It used to be
        // exempt, on the reasoning that a function reaches a module binding through the module
        // frame -- true within one module, and wrong across two: the frame a `LoadGlobal` finds
        // belongs to whichever module is *running*, not to the one the function was written in.
        // An exported function reading its own module's `const` read the importer's slot of the
        // same number instead, which is a wrong answer rather than a missing one.
        let captured = self.captured.contains(name) || self.cell_everything;
        let celled = captured;
        self.scopes[scope_index].insert(
            name.to_owned(),
            Local {
                slot,
                kind,
                imported: false,
                initialized,
                celled,
            },
        );
        // The cell has to exist before a hoisted function captures it, which is why this is here
        // rather than at the declaration.
        if celled {
            self.code.push(Instruction::NewCell(slot));
        }
        if initialized {
            // `var` reads as undefined before its declaration rather than throwing.
            self.push_constant(Constant::Undefined);
            self.code.push(if celled {
                Instruction::StoreCell(slot)
            } else {
                Instruction::StoreLocal(slot)
            });
        } else {
            // Keep the run-time state on every lexical slot, not only captured ones. A switch
            // shares one scope between all of its cases, so compiler-time `initialized` state
            // can be true after compiling one case even though another case jumps straight to a
            // read before the declaration. The marker lets the machine preserve the TDZ across
            // that control-flow join; captured cells need it for the same reason when a closure
            // is called too early.
            self.code.push(Instruction::MarkUninitialized(slot));
        }
    }

    /// Mark a name as past its dead zone, now that its declaration has run.
    fn initialize(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(local) = scope.get_mut(name) {
                local.initialized = true;
                return;
            }
        }
    }

    fn if_statement(
        &mut self,
        test: &Expression,
        consequent: &Statement,
        alternate: Option<&Statement>,
    ) {
        self.expression(test);
        let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
        self.branch_statement(consequent);
        if let Some(alternate) = alternate {
            let end_jump = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(false_jump);
            self.branch_statement(alternate);
            self.patch_jump(end_jump);
        } else {
            self.patch_jump(false_jump);
        }
    }

    /// One arm of an `if`, which is a statement rather than a block -- except when it is a
    /// function declaration, which Annex B B.3.4 says to read as though a block were written
    /// around it.
    ///
    /// The block matters: without it the declaration binds in whatever scope encloses the `if`,
    /// so `function(f) { if (true) function f() {} }` overwrote the *parameter* instead of
    /// creating a binding of its own and leaving the parameter alone, which is what B.3.2.1
    /// declines to do for a formal.
    fn branch_statement(&mut self, statement: &Statement) {
        if !matches!(statement.kind, StatementKind::Function(_)) {
            self.statement(statement);
            return;
        }
        self.scopes.push(HashMap::new());
        self.statement(statement);
        self.scopes.pop().expect("branch scope was just pushed");
    }

    /// `try`/`catch`/`finally`.
    fn statement_always_terminates(stmt: &Statement) -> bool {
        match &stmt.kind {
            StatementKind::Return(_) | StatementKind::Throw(_) => true,
            StatementKind::Block(stmts) => Self::statements_always_terminate(stmts),
            StatementKind::If {
                consequent,
                alternate: Some(alt),
                ..
            } => {
                Self::statement_always_terminates(consequent)
                    && Self::statement_always_terminates(alt)
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                if !finalizer.is_empty() && Self::statements_always_terminate(finalizer) {
                    return true;
                }
                if let Some(clause) = handler {
                    Self::statements_always_terminate(block)
                        && Self::statements_always_terminate(&clause.body)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn statements_always_terminate(stmts: &[Statement]) -> bool {
        stmts.iter().any(Self::statement_always_terminates)
    }

    ///
    /// The cleanup block is emitted once per exit path rather than once with a dispatcher.
    /// A `finally` has to run on every way out -- falling off the end, `return`, `break`,
    /// `continue`, and an exception -- and duplicating the block is what lets each of those stay
    /// an ordinary jump. It costs code size in exchange for no pending-completion register and
    /// no second dispatch mechanism; if the size ever matters, the shape here is what a
    /// dispatcher would replace.
    fn try_statement(
        &mut self,
        block: &[Statement],
        handler: Option<&CatchClause>,
        finalizer: &[Statement],
        span: Span,
    ) {
        let has_finalizer = !finalizer.is_empty();
        if has_finalizer {
            self.finalizers.push(Cleanup::Finally(finalizer.to_vec()));
        }

        let handler_jump = self.emit_jump(Instruction::PushHandler(0));
        self.handler_depth += 1;

        self.scopes.push(HashMap::new());
        self.statements(block);
        self.scopes.pop().expect("try scope was just pushed");
        self.code.push(Instruction::PopHandler);
        self.handler_depth -= 1;

        // Normal completion runs the cleanup and skips the handler.
        if has_finalizer {
            self.finalizers.pop();
            self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
        }
        let done_jump = self.emit_jump(Instruction::Jump(0));

        // The handler is entered with the thrown value on the stack.
        // Entering the handler pops it at runtime, so the count is already back to what it was.
        self.patch_jump(handler_jump);
        if has_finalizer {
            self.finalizers.push(Cleanup::Finally(finalizer.to_vec()));
        }
        match handler {
            Some(clause) => {
                let skip_catch_jump = self.emit_jump(Instruction::JumpIfReturnSignal(0));

                self.scopes.push(HashMap::new());
                match &clause.binding {
                    Some(name) => self.declare_bound(VariableKind::Let, name, span),
                    // `catch { }` discards the value rather than binding it.
                    None => self.code.push(Instruction::Pop),
                }

                let catch_handler_jump = if has_finalizer {
                    let j = self.emit_jump(Instruction::PushHandler(0));
                    self.handler_depth += 1;
                    Some(j)
                } else {
                    None
                };

                self.statements(&clause.body);

                if catch_handler_jump.is_some() {
                    self.code.push(Instruction::PopHandler);
                    self.handler_depth -= 1;
                }

                self.scopes.pop().expect("catch scope was just pushed");
                if has_finalizer {
                    self.finalizers.pop();
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                }

                if let Some(j) = catch_handler_jump {
                    let catch_done_jump = self.emit_jump(Instruction::Jump(0));

                    // Catch block threw: landing pad for `catch_handler_jump`.
                    // The thrown value from catch is on the value stack.
                    self.patch_jump(j);
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                    if !Self::statements_always_terminate(finalizer) {
                        self.code.push(Instruction::Throw);
                    }
                    let catch_landing_done = self.emit_jump(Instruction::Jump(0));

                    // Return signal skipped catch: landing pad for `skip_catch_jump`.
                    self.patch_jump(skip_catch_jump);
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                    if !Self::statements_always_terminate(finalizer) {
                        self.code.push(Instruction::Throw);
                    }
                    self.patch_jump(catch_landing_done);

                    self.patch_jump(catch_done_jump);
                } else {
                    let catch_done_jump = self.emit_jump(Instruction::Jump(0));
                    self.patch_jump(skip_catch_jump);
                    self.code.push(Instruction::Throw);
                    self.patch_jump(catch_done_jump);
                }
            }
            None => {
                // `try`/`finally` with no catch: run the cleanup, then let the exception
                // continue outwards. The value is still on the stack for `Throw` to take.
                if has_finalizer {
                    self.finalizers.pop();
                    self.emit_finalizer(&Cleanup::Finally(finalizer.to_vec()));
                }
                if !Self::statements_always_terminate(finalizer) {
                    self.code.push(Instruction::Throw);
                }
            }
        }

        self.patch_jump(done_jump);
    }

    /// Emit a cleanup block inline, with the enclosing cleanups temporarily out of scope.
    ///
    /// A `return` inside a `finally` must not re-run the same `finally`, which is what would
    /// happen if it were still on the stack while its own body compiled.
    fn emit_finalizer(&mut self, cleanup: &Cleanup) {
        match cleanup {
            Cleanup::Finally(statements) => {
                self.scopes.push(HashMap::new());
                let body = statements.clone();
                self.without_completion(|compiler| compiler.statements(&body));
                self.scopes.pop().expect("finalizer scope was just pushed");
            }
            Cleanup::CloseIterator { binding } => self.emit_iterator_close(&binding.clone()),
            Cleanup::DisposeResources(slots) => self.emit_disposals(&slots.clone()),
        }
    }

    /// Record the value a `using` declaration just bound, so the end of the scope disposes of it.
    ///
    /// The method is read here rather than at the disposal, which is what the specification's
    /// `GetDisposeMethod` says and is observable twice over: an accessor runs once, and a value
    /// with no disposal method is refused where it was declared instead of much later, when the
    /// program has already used it and cannot do anything about it.
    fn register_resource(&mut self, name: &str, awaiting: bool, span: Span) {
        let Some(scope) = self.resource_scopes.last_mut() else {
            self.error(Some(span), "a `using` declaration outside any scope");
            return;
        };
        let Some(slot) = scope.slots.get(scope.next).cloned() else {
            self.error(
                Some(span),
                "more `using` declarations than this scope counted",
            );
            return;
        };
        scope.next += 1;
        debug_assert_eq!(slot.awaiting, awaiting);

        self.load_lexically(name);
        self.code.push(Instruction::Dup);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // `using x = null` binds and disposes of nothing, and never looks for a method: a
        // missing resource is the one case the language lets pass.
        self.code.push(Instruction::Pop);
        let done = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(present);
        self.store_hidden(&slot.value);
        let dispose = self.constant_index(Constant::String("@@dispose".to_owned()));
        if awaiting {
            let asynchronous = self.constant_index(Constant::String("@@asyncDispose".to_owned()));
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(asynchronous));
            self.code.push(Instruction::Dup);
            let found = self.emit_jump(Instruction::JumpIfNotNullish(0));
            // `await using` falls back to the synchronous method, whose result is awaited like
            // any other -- which is what lets one object serve both forms.
            self.code.push(Instruction::Pop);
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(dispose));
            self.patch_jump(found);
        } else {
            self.load_hidden(&slot.value);
            self.code.push(Instruction::GetProperty(dispose));
        }

        self.code.push(Instruction::Dup);
        let callable = self.emit_jump(Instruction::JumpIfNotNullish(0));
        self.code.push(Instruction::Pop);
        self.emit_type_error(if awaiting {
            "the value is not asynchronously disposable"
        } else {
            "the value is not disposable"
        });
        self.patch_jump(callable);
        self.store_hidden(&slot.method);
        self.patch_jump(done);
    }

    /// Throw a `TypeError` the compiler decided on, built from the realm's own constructor.
    ///
    /// Through `LoadBuiltin` rather than by naming `TypeError`, because a program is free to
    /// shadow that name and the error a language rule raises is not the shadowing program's to
    /// choose.
    fn emit_type_error(&mut self, message: &str) {
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(message.to_owned()));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
    }

    /// Dispose of a scope's `using` resources, in reverse declaration order.
    ///
    /// A slot whose method binding is still empty belongs to a declaration this exit never
    /// reached -- an early `return` above it, or a throw from its own initialiser -- and there is
    /// nothing to dispose of.
    fn emit_disposals(&mut self, slots: &[ResourceSlot]) {
        for slot in slots.iter().rev() {
            self.load_hidden(&slot.method);
            let skip = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_hidden(&slot.value);
            self.load_hidden(&slot.method);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            if slot.awaiting {
                self.code.push(Instruction::Await);
            }
            // The specification discards what a disposal returns; only whether it threw matters.
            self.code.push(Instruction::Pop);
            self.patch_jump(skip);
        }
    }

    /// `if (it.return) it.return()` against a hidden `for-of` binding.
    ///
    /// The method is optional -- an array iterator has none -- so it is tested before it is
    /// called. The binding is loaded twice rather than duplicated on the stack because the test
    /// consumes what it reads, and a reload of a local is cheaper than the shuffling that
    /// keeping the receiver around would need.
    fn emit_iterator_close(&mut self, binding: &str) {
        let name = self.constant_index(Constant::String("return".to_owned()));
        self.load_hidden(binding);
        self.code.push(Instruction::GetProperty(name));
        let skip = self.emit_jump(Instruction::JumpIfFalse(0));
        self.load_hidden(binding);
        self.code.push(Instruction::CallMethod { name, argc: 0 });
        // The specification ignores the result unless it is not an object; nothing observes it
        // here, so it is discarded.
        self.code.push(Instruction::Pop);
        self.patch_jump(skip);
    }

    /// Run every cleanup between here and `depth`, innermost first.
    fn unwind_finalizers(&mut self, depth: usize) {
        while self.finalizers.len() > depth {
            let finalizer = self
                .finalizers
                .pop()
                .expect("checked by the loop condition");
            self.emit_finalizer(&finalizer);
            self.pending_finalizers.push(finalizer);
        }
        while let Some(finalizer) = self.pending_finalizers.pop() {
            self.finalizers.push(finalizer);
        }
    }

    /// `label: body`.
    ///
    /// A label on a loop names that loop, so it is handed to the loop's own context and nothing
    /// is emitted here. On anything else it names a block, which needs a context of its own for
    /// `break label` to have somewhere to land.
    fn labelled_statement(&mut self, label: &str, body: &Statement) {
        if matches!(
            body.kind,
            StatementKind::While { .. }
                | StatementKind::DoWhile { .. }
                | StatementKind::For { .. }
                | StatementKind::ForIn { .. }
                | StatementKind::ForOf { .. }
                | StatementKind::Switch { .. }
        ) {
            self.pending_label = Some(label.to_owned());
            self.statement(body);
            // Cleared in case the statement turned out not to claim it after all.
            self.pending_label = None;
            return;
        }

        self.loops.push(LoopContext {
            kind: BreakableKind::Labelled,
            label: Some(label.to_owned()),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this label pushed its own context");
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `for (const name in object) body`.
    ///
    /// Walks a snapshot of the keys by index rather than through the iterator protocol, because
    /// there is no iterator involved: the keys are produced once and the loop steps through them.
    /// Each key is rechecked against the object before the body runs, so one the body deleted is
    /// skipped -- the reason this is not simply a `for-of` over `Object.keys(object)`.
    fn for_in(
        &mut self,
        kind: VariableKind,
        name: &str,
        object: &Expression,
        body: &Statement,
        span: Span,
    ) {
        let subject = format!("%in{}", self.hidden_bindings);
        let keys = format!("%keys{}", self.hidden_bindings);
        let cursor = format!("%at{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());
        self.expression(object);
        self.declare_hidden(&subject, span);
        self.load_hidden(&subject);
        self.code.push(Instruction::EnumerateKeys);
        self.declare_hidden(&keys, span);
        self.push_constant(Constant::Number(0f64.to_bits()));
        self.declare_hidden(&cursor, span);

        let start = self.code.len();
        self.load_hidden(&cursor);
        self.load_hidden(&keys);
        let length = self.constant_index(Constant::String("length".to_owned()));
        self.code.push(Instruction::GetProperty(length));
        self.code.push(Instruction::Binary(BinaryOp::LessThan));
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));

        // Read this step's key, then advance the cursor, so a `continue` in the body cannot
        // re-read it.
        let key = format!("%key{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.load_hidden(&keys);
        self.load_hidden(&cursor);
        self.code.push(Instruction::GetComputed);
        self.declare_hidden(&key, span);
        self.load_hidden(&cursor);
        self.push_constant(Constant::Number(1f64.to_bits()));
        self.code.push(Instruction::Binary(BinaryOp::Add));
        self.store_hidden(&cursor);

        // A key the body has already deleted is not visited.
        self.load_hidden(&key);
        self.load_hidden(&subject);
        self.code.push(Instruction::Binary(BinaryOp::In));
        let skip = self.emit_jump(Instruction::JumpIfFalse(0));

        // A fresh binding each iteration, so a closure made in the body captures that step's key.
        self.scopes.push(HashMap::new());
        self.load_hidden(&key);
        self.declare_bound(kind, name, span);

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");
        self.scopes.pop().expect("iteration scope was just pushed");

        let continue_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, continue_target);
        }
        // A skipped key rejoins here, having already advanced the cursor.
        self.patch_jump(skip);
        self.code.push(Instruction::Jump(start as u32));

        self.patch_jump(exit_jump);
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// `for (const name of iterable) body`.
    ///
    /// Lowered to the protocol rather than to indexed access: the subject may be any iterable,
    /// and indexing an array directly would be wrong for everything else and would miss the
    /// re-read of `length` that mutation during iteration depends on.
    ///
    /// The iterator and each step's result live in hidden locals whose names carry a character an
    /// identifier cannot contain, so they can never collide with the program's own bindings.
    fn for_of(
        &mut self,
        kind: VariableKind,
        pattern: &Pattern,
        iterable: &Expression,
        body: &Statement,
        awaiting: bool,
        span: Span,
    ) {
        if awaiting && !self.compiling_function() && !self.module_body {
            // `for await` at top level is top-level `await` under another name, and a script is
            // not an evaluation anything can resume.
            self.error(
                Some(span),
                "top-level `for await` is only available in a module",
            );
            return;
        }
        let iterator_binding = format!("%iter{}", self.hidden_bindings);
        let result_binding = format!("%step{}", self.hidden_bindings);
        let asynchronous_binding = format!("%async{}", self.hidden_bindings);
        let next_binding = format!("%next{}", self.hidden_bindings);
        let method_binding = format!("%method{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        if awaiting {
            // `for await` prefers the async iterator and falls back to the sync one, and the two
            // are awaited in different places: an async iterator's `next` returns a promise of
            // the result, while a sync one returns the result directly and it is each *value*
            // that may be a promise. Which was found is remembered rather than re-derived.
            let subject = format!("%iterable{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.expression(iterable);
            self.declare_hidden(&subject, span);

            // `GetMethod(obj, @@asyncIterator)` is *one* read, and the method it answers with is
            // the one that gets called. Reading the symbol to decide and then reading it again to
            // call it asked an accessor twice and let a program hand back two different methods,
            // one of which was never invoked. It also missed `null`, which `GetMethod` treats as
            // absent exactly as it treats `undefined`.
            let asynchronous = self.constant_index(Constant::String("@@asyncIterator".to_owned()));
            self.load_hidden(&subject);
            self.code.push(Instruction::GetProperty(asynchronous));
            self.declare_hidden(&method_binding, span);

            self.load_hidden(&method_binding);
            let use_async = self.emit_jump(Instruction::JumpIfNotNullish(0));

            self.load_hidden(&subject);
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.push_constant(Constant::Boolean(false));
            let chosen = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(use_async);
            self.load_hidden(&subject);
            self.load_hidden(&method_binding);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            self.push_constant(Constant::Boolean(true));

            self.patch_jump(chosen);
            self.declare_hidden(&asynchronous_binding, span);
            self.declare_hidden(&iterator_binding, span);
        } else {
            // `iterable[Symbol.iterator]()`, which is what makes any iterable work rather than
            // only an array.
            self.expression(iterable);
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.declare_hidden(&iterator_binding, span);
        }

        // `GetIteratorFromMethod` reads `next` once and the record keeps it, so every step calls
        // the method the opening captured. Reading it per step is a `Get` the language does not
        // perform: a three-element loop asked a proxy four times where it asks once, and an
        // accessor that answers differently the second time was followed rather than pinned.
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.load_hidden(&iterator_binding);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_binding, span);

        // One handler for the whole loop rather than one per iteration: the guarded region is
        // the same every time round, and pushing it once keeps `continue` an ordinary jump.
        let handler_jump = self.emit_jump(Instruction::PushHandler(0));

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&next_binding);
        self.code.push(Instruction::NewArray { count: 0 });
        self.code.push(Instruction::Apply);
        if awaiting {
            // An async iterator hands back a promise of the result object; a sync one hands back
            // the object itself and has nothing to wait for here.
            self.load_hidden(&asynchronous_binding);
            let ready = self.emit_jump(Instruction::JumpIfFalse(0));
            self.code.push(Instruction::Await);
            self.patch_jump(ready);
        }
        self.guard_iterator_result();
        self.declare_hidden(&result_binding, span);

        self.load_hidden(&result_binding);
        let done = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(done));
        let exit_jump = self.emit_jump(Instruction::JumpIfTruthy(0));

        // A fresh binding each iteration, so a closure made in the body captures that step's
        // value rather than the last one.
        self.scopes.push(HashMap::new());
        self.load_hidden(&result_binding);
        let value = self.constant_index(Constant::String("value".to_owned()));
        self.code.push(Instruction::GetProperty(value));
        if awaiting {
            // The other half of the rule: a sync iterator's values are awaited one by one, which
            // is what makes `for await (const x of promises)` work on a plain array.
            self.load_hidden(&asynchronous_binding);
            let ready = self.emit_jump(Instruction::JumpIfTruthy(0));
            self.code.push(Instruction::Await);
            self.patch_jump(ready);
        }
        // A pattern, so `for (const [key, value] of entries)` binds both -- through the same
        // `bind_pattern` a declaration uses, defaults and nesting included.
        self.bind_pattern(kind, pattern, span);

        // Leaving the loop early has to close the iterator. Registering that as a cleanup is
        // what gets `return` right for free; `break` and an exception are handled by the two
        // landing pads below, which is why the loop's own context is recorded *above* it.
        self.finalizers.push(Cleanup::CloseIterator {
            binding: iterator_binding.clone(),
        });
        self.handler_depth += 1;
        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");
        self.handler_depth -= 1;
        self.finalizers.pop().expect("the close was just pushed");
        self.scopes.pop().expect("iteration scope was just pushed");

        // `continue` stays inside the loop, so it neither closes the iterator nor pops the guard.
        let continue_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, continue_target);
        }
        self.code.push(Instruction::Jump(start as u32));

        // Running out of values is a normal completion: the iterator closes itself, and closing
        // it again here would call `return` on an exhausted iterator.
        self.patch_jump(exit_jump);
        self.code.push(Instruction::PopHandler);
        let normal_exit = self.emit_jump(Instruction::Jump(0));

        if !context.breaks.is_empty() {
            let break_pad = self.code.len() as u32;
            for jump in context.breaks {
                self.patch_jump_to(jump, break_pad);
            }
            self.code.push(Instruction::PopHandler);
            self.emit_iterator_close(&iterator_binding);
            let leave = self.emit_jump(Instruction::Jump(0));
            self.emit_catch_pad(handler_jump, &iterator_binding);
            self.patch_jump(leave);
        } else {
            self.emit_catch_pad(handler_jump, &iterator_binding);
        }
        self.patch_jump(normal_exit);
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// The landing pad an exception thrown inside a `for-of` body reaches: close the iterator,
    /// then let the exception carry on outwards. Entering it has already popped the handler.
    fn emit_catch_pad(&mut self, handler_jump: usize, binding: &str) {
        self.patch_jump(handler_jump);
        self.emit_iterator_close(binding);
        self.code.push(Instruction::Throw);
    }

    /// `yield* iterable`.
    ///
    /// Compiled as a loop over the iterator protocol rather than as one instruction. That is not
    /// an optimisation, it is the only way to get the semantics: each value has to be yielded
    /// *and suspended on*, and the value the caller sends must be handed to the delegate's own
    /// `next`. A single instruction cannot do that, because the run loop cannot suspend part-way
    /// through evaluating one -- an earlier attempt drained the iterable up front, which is
    /// wrong for a two-way protocol and never terminates on an infinite one.
    ///
    /// The shape is the specification's:
    ///
    /// ```text
    /// iterator = iterable[Symbol.iterator]()
    /// sent = undefined
    /// loop { step = iterator.next(sent); if step.done { break step.value } sent = yield step.value }
    /// ```
    fn yield_delegate(&mut self, value: Option<&Expression>, span: Span) {
        let iterator_binding = format!("%delegate{}", self.hidden_bindings);
        let sent_binding = format!("%sent{}", self.hidden_bindings);
        let step_binding = format!("%step{}", self.hidden_bindings);
        let wrapped_binding = format!("%wrapped{}", self.hidden_bindings);
        let subject_binding = format!("%subject{}", self.hidden_bindings);
        let next_binding = format!("%next{}", self.hidden_bindings);
        let method_binding = format!("%method{}", self.hidden_bindings);
        self.hidden_bindings += 1;

        self.scopes.push(HashMap::new());

        // Whether each step has to be awaited, and where. Fixed at compile time by the kind of
        // generator this `yield*` was written in, exactly as `GetGeneratorKind` fixes it.
        let asynchronous = self.async_generator;
        let symbol = self.constant_index(Constant::String("@@iterator".to_owned()));
        let value_key = self.constant_index(Constant::String("value".to_owned()));

        match value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        if asynchronous {
            // `GetIterator(value, async)`: `@@asyncIterator` decides, and `@@iterator` is only
            // reached when it is absent. A present-but-not-callable `@@asyncIterator` is a
            // TypeError raised *without* ever touching `@@iterator` -- which is what the two
            // must not share a path for.
            self.declare_hidden(&subject_binding, span);
            let asynchronous_key =
                self.constant_index(Constant::String("@@asyncIterator".to_owned()));
            // Read once and kept: `GetMethod` performs a single `Get`, and the method it answers
            // with is the one that is called. Reading the symbol to decide and again to call it
            // ran an accessor twice and let a program hand back a method that was never invoked.
            self.load_hidden(&subject_binding);
            self.code.push(Instruction::GetProperty(asynchronous_key));
            self.declare_hidden(&method_binding, span);
            self.load_hidden(&method_binding);
            let is_async = self.emit_jump(Instruction::JumpIfNotNullish(0));

            // Absent: the sync iterator, whose steps the specification wraps in an
            // async-from-sync iterator. The wrapper is not materialised -- nothing here hands
            // it to the program -- but the two places it awaits are, which is the whole of what
            // it does that the async protocol does not.
            self.load_hidden(&subject_binding);
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.push_constant(Constant::Boolean(true));
            let chosen = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(is_async);
            self.load_hidden(&subject_binding);
            self.load_hidden(&method_binding);
            self.code.push(Instruction::NewArray { count: 0 });
            self.code.push(Instruction::Apply);
            self.push_constant(Constant::Boolean(false));

            self.patch_jump(chosen);
            self.declare_hidden(&wrapped_binding, span);
            self.declare_hidden(&iterator_binding, span);
        } else {
            self.code.push(Instruction::CallMethod {
                name: symbol,
                argc: 0,
            });
            self.declare_hidden(&iterator_binding, span);
        }

        // The iterator record's `next`, read once when the delegate was opened. Every step calls
        // what was captured, which is what the specification does and what a re-read per step
        // could not be: `yield*` over three values asked for `next` four times.
        let next = self.constant_index(Constant::String("next".to_owned()));
        self.load_hidden(&iterator_binding);
        self.code.push(Instruction::GetProperty(next));
        self.declare_hidden(&next_binding, span);

        self.push_constant(Constant::Undefined);
        self.declare_hidden(&sent_binding, span);

        let start = self.code.len();
        self.load_hidden(&iterator_binding);
        self.load_hidden(&next_binding);
        self.load_hidden(&sent_binding);
        self.code.push(Instruction::NewArray { count: 1 });
        self.code.push(Instruction::Apply);
        if asynchronous {
            // An async iterator answers with a promise of the step, so the step object itself
            // is what has to be awaited. A wrapped sync iterator answers with the step directly
            // and it is each *value* that may be a promise, which is awaited further down.
            self.load_hidden(&wrapped_binding);
            let sync_step = self.emit_jump(Instruction::JumpIfTruthy(0));
            self.code.push(Instruction::Await);
            self.patch_jump(sync_step);
        }
        self.guard_iterator_result();
        self.declare_hidden(&step_binding, span);

        self.load_hidden(&step_binding);
        let done = self.constant_index(Constant::String("done".to_owned()));
        self.code.push(Instruction::GetProperty(done));
        let not_done = self.emit_jump(Instruction::JumpIfFalse(0));

        // Done: load step.value and jump to finished
        self.load_hidden(&step_binding);
        self.code.push(Instruction::GetProperty(value_key));
        let finished_jump = self.emit_jump(Instruction::Jump(0));

        // Not done: yield the inner step object out directly, delegating return and throw completions.
        self.patch_jump(not_done);
        self.load_hidden(&iterator_binding);
        self.load_hidden(&step_binding);
        let yield_star_idx = self.emit_jump(Instruction::YieldStar(0));
        self.store_hidden(&sent_binding);
        self.code.push(Instruction::Jump(start as u32));

        // Finished: either normal completion from done step, or inner throw() returning done: true.
        // The final value of yield* is left on top of the stack.
        let finished_target = self.code.len() as u32;
        self.patch_jump(finished_jump);
        self.patch_jump_to(yield_star_idx, finished_target);
        if asynchronous {
            self.await_wrapped_value(&wrapped_binding);
        }

        self.scopes.pop().expect("delegate scope was just pushed");
    }

    /// Await the value on top of the stack, but only when the delegate is a wrapped sync
    /// iterator.
    ///
    /// `AsyncFromSyncIteratorContinuation` resolves each value before it becomes the step, so a
    /// sync iterator of promises yielded through an async `yield*` hands on their values. A real
    /// async iterator has already settled its step by this point and awaiting again would cost a
    /// tick the specification does not spend.
    fn await_wrapped_value(&mut self, wrapped_binding: &str) {
        self.load_hidden(wrapped_binding);
        let plain = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::Await);
        self.patch_jump(plain);
    }

    /// Throw a `TypeError` unless the value on top of the stack is an object.
    ///
    /// `IteratorNext` requires the result to be an Object, and the two compiled walks -- `for...of`
    /// and `yield*` -- read `done` off it with `GetProperty`. A primitive answers `undefined`
    /// there, which is falsy, which reads as "not done": `for (const x of { [Symbol.iterator]:
    /// () => ({ next: () => 1 }) })` looped for ever, with no diagnostic and no way out. A hang
    /// is the worst answer available, and the one shape that gives a program no thread to pull.
    ///
    /// Emitted rather than made an instruction, because an instruction has to be implemented in
    /// the C backend too, and everything this needs already exists there.
    ///
    /// The value is left on the stack.
    fn guard_iterator_result(&mut self) {
        // `typeof v === "object"` is true for null as well, so the null case is excluded by the
        // second test rather than by the first.
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let object_like = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.emit_iterator_result_type_error();
        self.patch_jump(object_like);
        self.patch_jump(callable);
        // `typeof null` is "object", so the null case reaches here and is rejected separately.
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::Null);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let not_null = self.emit_jump(Instruction::JumpIfFalse(0));
        self.emit_iterator_result_type_error();
        self.patch_jump(not_null);
    }

    /// `return v` from a *derived* constructor, which `[[Construct]]` does not simply hand back.
    ///
    /// An object is the override and wins outright. `undefined` yields `this` instead -- and
    /// reading `this` is what makes returning before `super()` a ReferenceError, exactly as the
    /// bare `return` above relies on. Anything else is a TypeError: the specification allows a
    /// derived constructor to replace its instance with an object, not to answer a primitive.
    /// `null` is a primitive here, which is why the `typeof` test alone will not do.
    ///
    /// Emitted rather than made an instruction, for the reason `this_binding` itself is a
    /// binding: the C backend implements nothing new, so the two engines cannot disagree.
    fn guard_derived_return(&mut self, binding: &str) {
        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("function".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let callable = self.emit_jump(Instruction::JumpIfTruthy(0));

        self.code.push(Instruction::Dup);
        self.code.push(Instruction::Unary(UnaryOp::TypeOf));
        self.push_constant(Constant::String("object".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let primitive = self.emit_jump(Instruction::JumpIfFalse(0));

        // `typeof null` is "object", so null reaches here and falls through to the primitive
        // handling below rather than counting as the object override.
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::Null);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let object_like = self.emit_jump(Instruction::JumpIfFalse(0));

        self.patch_jump(primitive);
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::Undefined);
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let refused = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::Pop);
        self.load_this_binding(binding);
        let yielded_this = self.emit_jump(Instruction::Jump(0));

        self.patch_jump(refused);
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "a derived constructor may only return an object or undefined".to_owned(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);

        self.patch_jump(callable);
        self.patch_jump(object_like);
        self.patch_jump(yielded_this);
    }

    /// A static class element may not be named `prototype`, which is a TypeError rather than a
    /// quiet overwrite: the constructor's `prototype` is what its instances inherit from, and a
    /// field that replaced it would leave every instance of the class descending from something
    /// the class never made.
    ///
    /// The name is converted first, so a key object whose `toString` answers "prototype" is
    /// refused for the same reason a literal one is. The constructor is already beneath the key
    /// on the stack, which is the shape `ToPropertyKey` reads.
    fn guard_static_prototype_key(&mut self) {
        self.code.push(Instruction::ToPropertyKey);
        self.code.push(Instruction::Dup);
        self.push_constant(Constant::String("prototype".to_owned()));
        self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
        let usable = self.emit_jump(Instruction::JumpIfFalse(0));
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "a static class element may not be named `prototype`".to_owned(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
        self.patch_jump(usable);
    }

    /// Read a hidden binding that an inner function captured, the way the home object is read.
    ///
    /// `load_hidden` resolves a local and nothing else, which is right where the binding and the
    /// reader are the same function. A private method is made where the class is defined and
    /// installed from inside the constructor, so the read crosses a function boundary and has to
    /// go through `resolve_or_capture`.
    fn load_captured_binding(&mut self, binding: &str) {
        match self.resolve_or_capture(binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot));
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, format!("`{binding}` went out of scope"));
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// `throw new TypeError("...")`, spelled in the instructions that already exist.
    fn emit_iterator_result_type_error(&mut self) {
        self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
        self.push_constant(Constant::String(
            "iterator result is not an object".to_owned(),
        ));
        self.code.push(Instruction::New { argc: 1 });
        self.code.push(Instruction::Throw);
    }

    fn store_hidden(&mut self, name: &str) {
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::StoreCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::StoreLocal(local.slot)),
            _ => self.error(None, format!("`{name}` went out of scope")),
        }
    }

    /// Bind a hidden local to the value on top of the stack.
    ///
    /// Reuses the slot when the name is already bound in this scope, because a compiler-
    /// introduced binding inside a loop body is declared once and assigned on every pass.
    fn declare_hidden(&mut self, name: &str, span: Span) {
        if matches!(self.resolve(name), Resolution::Local(_)) {
            self.store_hidden(name);
            return;
        }
        self.declare_bound(VariableKind::Let, name, span);
    }

    /// Declare `name` and store the value on top of the stack into it.
    fn declare_bound(&mut self, kind: VariableKind, name: &str, span: Span) {
        self.variable_with(kind, name, None, span, true);
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::StoreCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::StoreLocal(local.slot)),
            Resolution::GlobalObject => self.store_global_object_binding(name),
            _ => self.error(Some(span), format!("could not bind `{name}`")),
        }
    }

    fn load_hidden(&mut self, name: &str) {
        match self.resolve(name) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            _ => self.error(None, format!("`{name}` went out of scope")),
        }
    }

    /// `for (init; test; update) body`.
    ///
    /// The initialiser gets its own scope so a `let` there belongs to the loop rather than to
    /// the enclosing block, and `continue` jumps to the update rather than to the test -- which
    /// is the difference a `while` desugaring would silently lose.
    ///
    /// A `let` in the head is also re-bound on every pass, which is what makes
    /// `for (let i = 0; i < 3; i++) fs.push(() => i)` collect three closures over 0, 1 and 2
    /// rather than three over the value the loop ended on.
    fn for_statement(
        &mut self,
        init: Option<&Statement>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
        span: Span,
    ) {
        // A `using` in a `for` head belongs to the loop rather than to one pass through it: the
        // head runs once, and what it declares is disposed of when the loop is left -- by the
        // test failing, by a `break`, by a `return`, or by a throw.
        let resources = init.map_or_else(Vec::new, |init| {
            resource_declarations(std::slice::from_ref(init))
        });
        if resources.is_empty() {
            self.for_statement_body(init, test, update, body);
            return;
        }
        self.with_resource_scope(&resources, span, |compiler| {
            compiler.for_statement_body(init, test, update, body);
        });
    }

    fn for_statement_body(
        &mut self,
        init: Option<&Statement>,
        test: Option<&Expression>,
        update: Option<&Expression>,
        body: &Statement,
    ) {
        self.scopes.push(HashMap::new());
        if let Some(init) = init {
            // The head of a `for` is not a statement of the program, whatever it is spelled as
            // here: `eval("for (false; false; false);")` is `undefined`, not `false`. Only the
            // body can contribute a completion value.
            self.without_completion(|compiler| compiler.statement(init));
        }
        // Only a captured binding can tell one iteration's copy from another's, and only a
        // captured binding is boxed -- so the boxed `let`s of the head are exactly the set that
        // needs re-binding. Sorted by slot because the scope is a hash map and the bytecode is a
        // cache key: the same source must always produce the same instructions.
        let mut per_iteration: Vec<u32> = self
            .scopes
            .last()
            .expect("the loop scope was just pushed")
            .values()
            .filter(|local| local.celled && !matches!(local.kind, VariableKind::Var))
            .map(|local| local.slot)
            .collect();
        per_iteration.sort_unstable();

        let start = self.code.len();
        let exit_jump = test.map(|test| {
            self.expression(test);
            self.emit_jump(Instruction::JumpIfFalse(0))
        });

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` lands here rather than at the test, so the update still runs.
        let update_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, update_target);
        }
        // Re-bind before the update, not after: the update belongs to the *next* iteration, so
        // it must write the next iteration's cell and leave this one holding what the body saw.
        for slot in per_iteration {
            self.code.push(Instruction::LoadCell(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }
        if let Some(update) = update {
            self.expression(update);
            self.code.push(Instruction::Pop);
        }
        self.code.push(Instruction::Jump(start as u32));

        if let Some(exit_jump) = exit_jump {
            self.patch_jump(exit_jump);
        }
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("loop scope was just pushed");
    }

    /// `break` or `continue`, recorded for the enclosing loop to patch once its shape is known.
    fn jump_out(&mut self, label: Option<&str>, is_break: bool, span: Span) {
        // Only the cleanups entered since the target began: an outer `finally` is not left by
        // this jump, so running it here would run it twice.
        //
        // Unlabelled, `break` leaves the innermost breakable and `continue` steps over any
        // `switch` in the way, because a `switch` is not something a `continue` can continue.
        // Labelled, both look for that label instead -- which is the point of a label, and is
        // what lets one `break` leave several loops at once.
        let target = match label {
            None if is_break => self.loops.len().checked_sub(1),
            None => self
                .loops
                .iter()
                .rposition(|context| context.kind == BreakableKind::Loop),
            Some(label) => self.loops.iter().rposition(|context| {
                context.label.as_deref() == Some(label)
                    && (is_break || context.kind == BreakableKind::Loop)
            }),
        };
        if target.is_none()
            && let Some(label) = label
        {
            // The label is not there, or a `continue` named something that is not a loop.
            self.error(
                Some(span),
                format!("no enclosing loop or statement labelled `{label}`"),
            );
            return;
        }
        let (depth, handler_depth) = target.map_or((0, 0), |index| {
            let context = &self.loops[index];
            (context.finalizer_depth, context.handler_depth)
        });
        // Before the cleanups, not after: a `finally` reached by `break` must not still be
        // guarded by the handler of the `try` it belongs to, or an exception it raises would be
        // caught by its own `catch`.
        for _ in handler_depth..self.handler_depth {
            self.code.push(Instruction::PopHandler);
        }
        self.unwind_finalizers(depth);

        let jump = self.emit_jump(Instruction::Jump(0));
        let Some(context) = target.map(|index| &mut self.loops[index]) else {
            self.error(
                Some(span),
                if is_break {
                    "`break` outside of a loop"
                } else {
                    "`continue` outside of a loop"
                },
            );
            return;
        };
        if is_break {
            context.breaks.push(jump);
        } else {
            context.continues.push(jump);
        }
    }

    fn patch_jump_to(&mut self, jump: usize, target: u32) {
        match &mut self.code[jump] {
            Instruction::Jump(slot)
            | Instruction::JumpIfFalse(slot)
            | Instruction::JumpIfTruthy(slot)
            | Instruction::JumpIfNotNullish(slot)
            // A handler's target is patched the same way a jump's is: it is not known until the
            // handler body has been emitted.
            | Instruction::PushHandler(slot)
            | Instruction::YieldStar(slot)
            | Instruction::JumpIfReturnSignal(slot) => *slot = target,
            other => unreachable!("patched a {other:?}, which is not a jump"),
        }
    }

    fn while_statement(&mut self, test: &Expression, body: &Statement) {
        let loop_start = self.code.len();
        self.expression(test);
        let exit_jump = self.emit_jump(Instruction::JumpIfFalse(0));

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` re-tests, which for a `while` is the top of the loop.
        for jump in context.continues {
            self.patch_jump_to(jump, loop_start as u32);
        }
        self.code.push(Instruction::Jump(loop_start as u32));
        self.patch_jump(exit_jump);
        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `do body while (test)`, whose body always runs once before the test is reached.
    fn do_while_statement(&mut self, body: &Statement, test: &Expression) {
        let loop_start = self.code.len();

        self.loops.push(LoopContext {
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        self.statement(body);
        let context = self.loops.pop().expect("this loop pushed its own context");

        // `continue` skips the rest of the body but still runs the test, so it lands here rather
        // than at the top -- the one place a do-while differs from a while for a jump.
        let test_target = self.code.len() as u32;
        for jump in context.continues {
            self.patch_jump_to(jump, test_target);
        }
        self.expression(test);
        let repeat = self.emit_jump(Instruction::JumpIfTruthy(0));
        self.patch_jump_to(repeat, loop_start as u32);

        let exit = self.code.len() as u32;
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
    }

    /// `switch (discriminant) { case … }`.
    ///
    /// Two passes over the arms, because a `switch` is not a set of alternatives: the tests are
    /// evaluated in order until one matches with `===`, and control then enters that arm's body
    /// and *keeps going* through the arms after it until a `break`. So the tests are emitted as a
    /// dispatch chain and the bodies as one straight run entered at different offsets.
    ///
    /// `default` is not special beyond being the arm with no test: it may sit anywhere among
    /// them, it is only reached once every test has failed, and it falls through like the rest.
    fn switch_statement(&mut self, discriminant: &Expression, cases: &[SwitchCase], span: Span) {
        // The discriminant is evaluated once, however many tests are compared against it.
        let subject = format!("%switch{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());
        self.expression(discriminant);
        self.declare_hidden(&subject, span);

        // Every arm shares the switch's scope, so the hoisting pass spans all of them -- a
        // function declared in one case is callable from another, which is what one scope means.
        //
        // Emitted here, before the dispatch chain, because the chain's jumps land *inside* the
        // bodies: a prologue placed after them is skipped by every path that reaches an arm.
        // That is invisible while hoisting only allocates slots, and is a wrong answer the moment
        // it emits anything -- a `NewCell` for a captured declaration, or the dead-zone marker a
        // `let` in a case needs.
        for case in cases {
            for statement in &case.body {
                self.hoist_names(statement);
            }
        }

        let mut entries = Vec::with_capacity(cases.len());
        let mut fallback = None;
        for case in cases {
            let Some(test) = &case.test else {
                // Emitted with the other bodies below; only its entry point is recorded here.
                fallback = Some(entries.len());
                entries.push(None);
                continue;
            };
            self.load_hidden(&subject);
            self.expression(test);
            self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
            entries.push(Some(self.emit_jump(Instruction::JumpIfTruthy(0))));
        }
        // No test matched, so control goes to `default` if there is one and past the whole
        // statement if there is not.
        let no_match = self.emit_jump(Instruction::Jump(0));

        self.loops.push(LoopContext {
            kind: BreakableKind::Switch,
            label: self.pending_label.take(),
            finalizer_depth: self.finalizers.len(),
            handler_depth: self.handler_depth,
            ..LoopContext::default()
        });
        let mut starts = Vec::with_capacity(cases.len());
        for case in cases {
            starts.push(self.code.len() as u32);
            for statement in &case.body {
                self.statement(statement);
            }
        }
        let context = self
            .loops
            .pop()
            .expect("this switch pushed its own context");

        for (entry, start) in entries.iter().zip(&starts) {
            if let Some(jump) = entry {
                self.patch_jump_to(*jump, *start);
            }
        }
        match fallback.and_then(|index| starts.get(index)) {
            Some(start) => self.patch_jump_to(no_match, *start),
            None => self.patch_jump(no_match),
        }

        let exit = self.code.len() as u32;
        // A `default` in the middle still falls through, so this is the only jump that leaves
        // early -- and it is the same jump `break` uses.
        for jump in context.breaks {
            self.patch_jump_to(jump, exit);
        }
        self.scopes.pop().expect("switch scope was just pushed");
    }

    fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.push_literal(literal),
            ExpressionKind::Identifier(name) => {
                self.with_scoped_load(name, |compiler| compiler.load_lexically(name));
            }
            // `typeof x` inside a `with` has to ask the object first, and only then fall back
            // to the rule below -- the name may be absent from every binding around it and still
            // be a property of the object.
            ExpressionKind::Unary {
                operator: UnaryOperator::TypeOf,
                argument,
            } if matches!(
                &argument.kind,
                ExpressionKind::Identifier(name) if !self.with_scopes_for(name).is_empty()
            ) =>
            {
                let ExpressionKind::Identifier(name) = &argument.kind else {
                    unreachable!("the guard above matched an identifier")
                };
                let unbound = self.names_nothing(name)
                    && Builtin::from_name(name).is_none()
                    && name != "undefined"
                    && !(name == "arguments" && self.compiling_function());
                self.with_scoped_load(name, |compiler| {
                    // The *value*, not the string: the `typeof` below is what turns it into one,
                    // and pushing an answer here would report `"string"`. A name no scope bound
                    // is asked of the global object at run time rather than answered now,
                    // because the realm may have gained it since this was compiled.
                    if unbound {
                        let name = compiler.constant_index(Constant::String(name.to_owned()));
                        compiler
                            .code
                            .push(Instruction::LoadGlobalBindingOrUndefined(name));
                    } else {
                        compiler.load_lexically(name);
                    }
                });
                self.code.push(Instruction::Unary(UnaryOp::TypeOf));
            }
            // `typeof undeclaredName` is "undefined" rather than a throw -- the one place a name
            // may be read without existing, and the whole reason feature detection works.
            ExpressionKind::Unary {
                operator: UnaryOperator::TypeOf,
                argument,
            } if matches!(
                &argument.kind,
                ExpressionKind::Identifier(name)
                    if self.names_nothing(name)
                        && Builtin::from_name(name).is_none()
                        && name != "undefined"
                        && name != "this"
                        && !(name == "arguments" && self.compiling_function())
            ) =>
            {
                let ExpressionKind::Identifier(name) = &argument.kind else {
                    unreachable!("the guard above matched an identifier")
                };
                // Asked of the global object rather than folded: `typeof x` where a `var` in
                // global code, an `eval`, or a plain `globalThis.x = 1` created `x` is not
                // `"undefined"`, and only the running realm knows.
                let name = self.constant_index(Constant::String(name.to_owned()));
                self.code
                    .push(Instruction::LoadGlobalBindingOrUndefined(name));
                self.code.push(Instruction::Unary(UnaryOp::TypeOf));
            }
            ExpressionKind::Unary { operator, argument } => {
                self.expression(argument);
                self.code.push(Instruction::Unary(match operator {
                    UnaryOperator::Plus => UnaryOp::Plus,
                    UnaryOperator::Negate => UnaryOp::Negate,
                    UnaryOperator::Not => UnaryOp::Not,
                    UnaryOperator::TypeOf => UnaryOp::TypeOf,
                    UnaryOperator::BitwiseNot => UnaryOp::BitwiseNot,
                    UnaryOperator::Void => UnaryOp::Void,
                }));
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left);
                self.expression(right);
                self.code.push(Instruction::Binary(match operator {
                    BinaryOperator::Add => BinaryOp::Add,
                    BinaryOperator::Subtract => BinaryOp::Subtract,
                    BinaryOperator::Multiply => BinaryOp::Multiply,
                    BinaryOperator::Divide => BinaryOp::Divide,
                    BinaryOperator::Remainder => BinaryOp::Remainder,
                    BinaryOperator::Exponentiate => BinaryOp::Exponentiate,
                    BinaryOperator::StrictEqual => BinaryOp::StrictEqual,
                    BinaryOperator::In => BinaryOp::In,
                    BinaryOperator::InstanceOf => BinaryOp::InstanceOf,
                    BinaryOperator::StrictNotEqual => BinaryOp::StrictNotEqual,
                    BinaryOperator::LooseEqual => BinaryOp::LooseEqual,
                    BinaryOperator::LooseNotEqual => BinaryOp::LooseNotEqual,
                    BinaryOperator::BitwiseAnd => BinaryOp::BitwiseAnd,
                    BinaryOperator::BitwiseOr => BinaryOp::BitwiseOr,
                    BinaryOperator::BitwiseXor => BinaryOp::BitwiseXor,
                    BinaryOperator::ShiftLeft => BinaryOp::ShiftLeft,
                    BinaryOperator::ShiftRight => BinaryOp::ShiftRight,
                    BinaryOperator::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
                    BinaryOperator::LessThan => BinaryOp::LessThan,
                    BinaryOperator::LessThanOrEqual => BinaryOp::LessThanOrEqual,
                    BinaryOperator::GreaterThan => BinaryOp::GreaterThan,
                    BinaryOperator::GreaterThanOrEqual => BinaryOp::GreaterThanOrEqual,
                }));
            }
            ExpressionKind::Logical {
                operator,
                left,
                right,
            } => self.logical(*operator, left, right),
            ExpressionKind::Assignment {
                operator,
                target,
                value,
            } => self.assignment(*operator, target, value, expression.span),
            ExpressionKind::PatternAssignment { pattern, value } => {
                // An assignment is an expression whose value is the right-hand side, so a copy
                // is kept back before the pattern consumes it.
                self.expression(value);
                self.code.push(Instruction::Dup);
                self.assign_pattern(pattern, expression.span);
            }
            ExpressionKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                self.expression(test);
                let false_jump = self.emit_jump(Instruction::JumpIfFalse(0));
                self.expression(consequent);
                let end_jump = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(false_jump);
                self.expression(alternate);
                self.patch_jump(end_jump);
            }
            ExpressionKind::Call {
                callee,
                arguments,
                optional,
            } => self.call(callee, arguments, *optional, expression.span),
            ExpressionKind::This => self.load_this(),
            // `undefined` in a field initialiser: the initialiser is its own function to the
            // language, and reading the frame's `new.target` there answered `C` for
            // `class C { x = new.target }` because Kiln emits the initialiser into the
            // constructor.
            ExpressionKind::NewTarget => {
                if self.field_initialiser {
                    self.push_constant(Constant::Undefined);
                } else {
                    self.code.push(Instruction::LoadNewTarget);
                }
            }
            ExpressionKind::Yield { delegate, value } if !*delegate => {
                match value {
                    Some(value) => self.expression(value),
                    None => self.push_constant(Constant::Undefined),
                }
                self.code.push(Instruction::Yield);
            }
            ExpressionKind::Yield { value, .. } => {
                self.yield_delegate(value.as_deref(), expression.span)
            }
            ExpressionKind::Class(class) => self.class_expression(class, expression.span),
            ExpressionKind::Update {
                increment,
                prefix,
                target,
            } => self.update(*increment, *prefix, target, expression.span),
            ExpressionKind::UpdateMember {
                increment,
                prefix,
                object,
                property,
            } => self.update_member(*increment, *prefix, object, property, expression.span),
            ExpressionKind::SuperCall { arguments } => {
                if Self::has_spread(arguments) {
                    // The count is the length of an array the program computes, which is exactly
                    // what `argc` cannot say. The parent is reached through the same runtime-list
                    // shape `Apply` uses, but through `SuperApply`, because running the parent
                    // against the instance being built is not an ordinary call.
                    self.load_super();
                    self.array_literal(arguments, expression.span);
                    self.code.push(Instruction::SuperApply);
                    self.store_this_binding();
                    self.emit_elements_after_super(expression.span);
                    return;
                }
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many arguments to `super`");
                    return;
                }
                self.load_super();
                for argument in arguments {
                    let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                        continue;
                    };
                    self.expression(value);
                }
                // The parent's return value is this expression's value, so `SuperCall` leaves
                // exactly one thing on the stack and needs no epilogue here.
                self.code.push(Instruction::SuperCall {
                    argc: arguments.len() as u16,
                });
                self.store_this_binding();
                self.emit_elements_after_super(expression.span);
            }
            // `#x in o` compiles to the ordinary `in`: a private field is stored under a key no
            // program can write, so asking whether the key is there *is* the brand check.
            ExpressionKind::PrivateIn { name, object } => {
                // The key alone is not the answer: two evaluations of one class share it, so an
                // instance of a *sibling* evaluation carries the name and must still answer
                // `false`. Unlike an access, this reports rather than throws -- `#x in o` exists
                // to ask the question without raising.
                let brand = self.private_brand(name);
                let brand_key = self.constant_index(Constant::String(self.private_brand_key(name)));
                let key = self.private_key(name);
                let held = format!("%pin{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                self.scopes.push(HashMap::new());
                self.expression(object);
                self.declare_hidden(&held, expression.span);
                self.push_constant(Constant::String(key));
                self.load_hidden(&held);
                self.code.push(Instruction::Binary(BinaryOp::In));
                let absent = self.emit_jump(Instruction::JumpIfFalse(0));
                self.load_hidden(&held);
                self.code.push(Instruction::GetProperty(brand_key));
                self.push_private_brand(&brand);
                self.code.push(Instruction::Binary(BinaryOp::StrictEqual));
                let done = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(absent);
                self.push_constant(Constant::Boolean(false));
                self.patch_jump(done);
                self.scopes.pop().expect("private-in scope was just pushed");
            }
            ExpressionKind::SuperMember { property } => {
                // `super.m` resolves on the prototype of the object this method was defined on,
                // not on `this` and not on the parent constructor -- but it is still a reference
                // *with* a `this`, so writing one before `super()` is the same ReferenceError.
                self.require_this_binding();
                self.load_home_prototype();
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::LoadSuperProperty(name));
            }
            ExpressionKind::SuperComputedMember { property } => {
                // The key first, then the prototype: `super[f()]` runs `f` before it asks the
                // home object what it inherits from.
                self.require_this_binding();
                self.expression(property);
                self.load_home_prototype();
                self.code.push(Instruction::LoadSuperElement);
            }
            ExpressionKind::Function(function) => {
                // A function expression the source gave a name to can call itself by that name,
                // and the name is visible nowhere else. The scope pushed here is what keeps it
                // there -- the same shape a named class expression uses, and for the same reason.
                let self_bound = function
                    .binds_own_name
                    .then(|| function.name.clone())
                    .flatten();
                if let Some(name) = &self_bound {
                    self.scopes.push(HashMap::new());
                    self.predeclare(VariableKind::Const, name, false);
                }
                if let Some(index) = self.compile_function(function, expression.span) {
                    self.code.push(Instruction::MakeFunction(index));
                }
                if let Some(name) = &self_bound {
                    // The closure is both the expression's value and what the binding holds, so
                    // it is duplicated rather than loaded back.
                    self.code.push(Instruction::Dup);
                    match self.resolve(name) {
                        Resolution::Local(local) if local.celled => {
                            self.code.push(Instruction::StoreCell(local.slot));
                        }
                        Resolution::Local(local) => {
                            self.code.push(Instruction::StoreLocal(local.slot));
                        }
                        _ => {
                            self.error(Some(expression.span), "could not bind function expression");
                            self.code.push(Instruction::Pop);
                        }
                    }
                    self.scopes.pop().expect("function scope was just pushed");
                }
            }
            ExpressionKind::Array { elements } => self.array_literal(elements, expression.span),
            ExpressionKind::Sequence { operands } => {
                let Some((last, rest)) = operands.split_last() else {
                    self.push_constant(Constant::Undefined);
                    return;
                };
                // Every operand is evaluated for its effects; only the last one is the value.
                for operand in rest {
                    self.expression(operand);
                    self.code.push(Instruction::Pop);
                }
                self.expression(last);
            }
            // `await` is a `yield` whose resumption the async driver supplies rather than the
            // program: the same suspension, resumed by a promise settling instead of a `next`.
            ExpressionKind::Await(value) => {
                if !self.compiling_function() && !self.module_body {
                    // A script has nowhere to put the suspension: only a module body is an
                    // evaluation the host can stop and resume, which is what makes top-level
                    // `await` a module-only form rather than a general one.
                    self.error(
                        Some(expression.span),
                        "top-level `await` is only available in a module",
                    );
                    self.push_constant(Constant::Undefined);
                    return;
                }
                self.expression(value);
                self.code.push(Instruction::Await);
            }
            // The specifier expression is evaluated like any other, so a `throw` inside it
            // throws. Only what happens *after* it -- coercing it to a string, resolving it,
            // evaluating the module -- rejects the promise instead, which is why the coercion
            // is the instruction's job and not something emitted around it.
            ExpressionKind::ImportCall { specifier, options } => {
                self.expression(specifier);
                // The second argument is evaluated in source order and then discarded: nothing
                // reads an import attribute yet, but a getter written in one still runs.
                if let Some(options) = options {
                    self.expression(options);
                    self.code.push(Instruction::DynamicImportWithOptions);
                } else {
                    self.code.push(Instruction::DynamicImport);
                }
            }
            ExpressionKind::Chain { expression: inner } => {
                self.chains.push(Vec::new());
                self.expression(inner);
                let exits = self.chains.pop().expect("this chain pushed its own frame");
                // Every `?.` in the chain lands here with `undefined` already on the stack, so
                // the chain's value is one thing either way.
                for jump in exits {
                    self.patch_jump(jump);
                }
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, expression.span);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
            }
            ExpressionKind::ComputedAssignment {
                operator,
                object,
                property,
                value,
            } => self.computed_assignment(*operator, object, property, value),
            ExpressionKind::New { callee, arguments } => {
                if arguments.len() > u16::MAX as usize {
                    self.error(Some(expression.span), "too many constructor arguments");
                    return;
                }
                if Self::has_spread(arguments) {
                    self.expression(callee);
                    self.array_literal(arguments, expression.span);
                    self.code.push(Instruction::ConstructSpread);
                    return;
                }
                self.expression(callee);
                for argument in arguments {
                    let ListElement::Item(value) = argument else {
                        unreachable!("just checked that there are no spreads");
                    };
                    self.expression(value);
                }
                self.code.push(Instruction::New {
                    argc: arguments.len() as u16,
                });
            }
            ExpressionKind::MemberAssignment {
                operator,
                object,
                property,
                value,
            } => self.member_assignment(*operator, object, property, value),
            ExpressionKind::SuperMemberAssignment {
                operator,
                property,
                value,
            } => self.super_member_assignment(*operator, property, value, expression.span),
            ExpressionKind::SuperComputedAssignment {
                operator,
                property,
                value,
            } => self.super_computed_assignment(*operator, property, value, expression.span),
            ExpressionKind::PrivateAssignment {
                operator,
                object,
                name,
                value,
            } => self.private_assignment(*operator, object, name, value),
            ExpressionKind::Object { members } => self.object_literal(members, expression.span),
            // Concatenation, starting from a string so that every `+` after it is a string
            // append rather than a numeric add. `${1}${2}` is "12" for exactly that reason.
            ExpressionKind::Template {
                quasis,
                expressions,
            } => {
                let mut quasis = quasis.iter();
                self.push_constant(Constant::String(quasis.next().cloned().unwrap_or_default()));
                for (expression, quasi) in expressions.iter().zip(quasis) {
                    // `${x}` is ToString(x), which reduces an object with the *string* hint --
                    // `toString` before `valueOf`. Concatenation reduces with the default hint,
                    // the other way round, so a template is not `"" + x`: for an object with a
                    // `valueOf` and no own `toString` the two genuinely disagree, and desugaring
                    // to `+` silently produced the wrong one.
                    //
                    // A literal is already a primitive and converts to itself, so it keeps the
                    // plain concatenation -- which is also what leaves a template of constants
                    // compilable by a backend that has no calls.
                    if matches!(expression.kind, ExpressionKind::Literal(_)) {
                        self.expression(expression);
                    } else {
                        self.code.push(Instruction::LoadBuiltin(Builtin::String));
                        self.expression(expression);
                        self.code.push(Instruction::Call { argc: 1 });
                    }
                    self.code.push(Instruction::Binary(BinaryOp::Add));
                    // A run of text between two holes is skipped when it is empty, which is the
                    // common shape: `${a}${b}` has two empty quasis in the middle and the end.
                    if !quasi.is_empty() {
                        self.push_constant(Constant::String(quasi.clone()));
                        self.code.push(Instruction::Binary(BinaryOp::Add));
                    }
                }
            }
            ExpressionKind::TemplateStrings { cooked, raw } => {
                if cooked.len() > u32::MAX as usize || raw.len() > u32::MAX as usize {
                    self.error(Some(expression.span), "template has too many segments");
                    return;
                }
                let site_index = u32::try_from(self.template_sites.len()).unwrap_or(u32::MAX);
                self.template_sites.push(TemplateSite {
                    cooked: cooked.clone(),
                    raw: raw.clone(),
                });
                self.code.push(Instruction::LoadTemplateObject(site_index));
            }
            ExpressionKind::RegExp { pattern, flags } => {
                let pattern = self.constant_index(Constant::String(pattern.clone()));
                let flags = self.constant_index(Constant::String(flags.clone()));
                self.code.push(Instruction::NewRegExp { pattern, flags });
            }
            ExpressionKind::PrivateMember {
                object,
                name,
                optional,
            } => {
                self.expression(object);
                // `o?.#x` on a nullish `o` leaves the chain with `undefined`, exactly as an
                // ordinary optional member read does. Without this the brand check ran on
                // `null` and raised the TypeError the `?.` was written to avoid.
                self.short_circuit_if_nullish(*optional, expression.span);
                self.load_private_brand(name);
                let name = self.constant_index(Constant::String(self.private_key(name)));
                self.code.push(Instruction::GetPrivate(name));
            }
            ExpressionKind::DeleteValue { argument } => {
                // The operand is evaluated and discarded: `delete f()` calls `f`, and the
                // answer is `true` whatever it returned.
                self.expression(argument);
                self.code.push(Instruction::Pop);
                self.push_constant(Constant::Boolean(true));
            }
            ExpressionKind::DeleteBinding { name } => self.delete_binding(name),
            ExpressionKind::Delete { object, property } => {
                self.expression(object);
                match property.as_ref() {
                    // `delete obj.#x` is a syntax error, so the frontend never builds one.
                    ObjectKey::Private(name) => self.error(
                        Some(expression.span),
                        format!("`delete` of the private name `#{name}`"),
                    ),
                    ObjectKey::Static(key) => {
                        let name = self.constant_index(Constant::String(key.clone()));
                        self.code.push(self.delete_property_instruction(name));
                    }
                    ObjectKey::Computed(key) => {
                        self.expression(key);
                        self.code.push(self.delete_computed_instruction());
                    }
                }
            }
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, expression.span);
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::GetProperty(name));
            }
            ExpressionKind::Unsupported { feature } => {
                self.error(
                    Some(expression.span),
                    format!("unsupported expression: {feature}"),
                );
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// Leave the enclosing optional chain with `undefined` when the value on top of the stack is
    /// nullish, or fall through with it untouched.
    ///
    /// Does nothing when the access is not optional, so every member read can call it and only
    /// the ones written with `?.` cost anything.
    fn short_circuit_if_nullish(&mut self, optional: bool, span: Span) {
        if !optional {
            return;
        }
        if self.chains.is_empty() {
            // The frontend only produces an optional access inside a chain, so this means the
            // two have drifted apart rather than that a program did something unusual.
            self.error(Some(span), "`?.` outside of an optional chain");
            return;
        }
        self.code.push(Instruction::Dup);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // Nullish: drop the object and hand the whole chain `undefined`.
        self.code.push(Instruction::Pop);
        self.push_constant(Constant::Undefined);
        let exit = self.emit_jump(Instruction::Jump(0));
        self.chains
            .last_mut()
            .expect("checked just above")
            .push(exit);
        self.patch_jump(present);
    }

    /// An object literal.
    ///
    /// With no spread this is one `NewObject` over the whole literal, which is the common case
    /// and the cheap one. A spread splits the literal into segments: each run of properties
    /// becomes its own object and is merged into the accumulator in source order, so a property
    /// written after a spread overwrites what the spread contributed and one written before it
    /// is overwritten. Merging a segment uses the same instruction the spread does, rather than
    /// a second way to set a property on an object already on the stack.
    fn object_literal(&mut self, members: &[ObjectMember], span: Span) {
        if members.len() > u32::MAX as usize {
            self.error(Some(span), "object literal has too many properties");
            return;
        }
        let mut pending = 0u32;
        let mut accumulating = false;
        // A method written in a literal is defined *on the literal*, so `super.x` inside one
        // reads the literal's own prototype. The binding is reserved before any member is
        // compiled, because a method closes over it and is compiled before the object it will
        // name exists; it is filled in once the literal is built, below.
        //
        // Only a method or an accessor gets one. `{ x: v }` and `{ ...o }` cannot contain a
        // `super` that belongs to this literal -- a `super` written in a value position belongs
        // to whatever method encloses the literal -- so a plain object literal emits exactly
        // what it emitted before.
        let home = members
            .iter()
            .any(|member| {
                matches!(
                    member,
                    ObjectMember::Accessor { .. }
                        | ObjectMember::Property(kiln_hir::ObjectProperty {
                            form: kiln_hir::PropertyForm::Method,
                            ..
                        })
                )
            })
            .then(|| self.declare_home_binding(span));

        for member in members {
            match member {
                // `{ __proto__: v }` is not a property at all: it links the object to `v`. Only
                // this spelling does -- `{ __proto__ }` and `{ __proto__() {} }` are ordinary own
                // properties, and so is `{ ["__proto__"]: v }`, because the specification asks
                // whether the key was computed rather than what it evaluated to.
                ObjectMember::Property(property)
                    if matches!(&property.key, ObjectKey::Static(key) if key == "__proto__")
                        && property.form == kiln_hir::PropertyForm::KeyValue =>
                {
                    // The prototype is set on an object, so the run of pairs written before it
                    // has to become one first -- the same reason an accessor flushes the run.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    self.expression(&property.value);
                    self.code.push(Instruction::SetPrototype);
                }
                ObjectMember::Property(property) => {
                    // `{ [k]: function () {} }` names the function after the key, which is only
                    // knowable once the key has been evaluated. The pairs waiting to become an
                    // object are flushed first so the object exists to define on -- the shape a
                    // spread already uses -- and the key is then evaluated in its own place, so
                    // the source order of its side effects is unchanged.
                    //
                    // The same path serves every pair written after a spread: the accumulator
                    // exists by then, so the pair is defined on it directly rather than built
                    // into a second object and spread in -- one literal, not two, and the
                    // spread's per-key copy is not run twice for `{ ...base, f: i }`.
                    let named_after_key = matches!(&property.key, ObjectKey::Computed(_))
                        && anonymous_function_value(&property.value);
                    if accumulating || named_after_key {
                        let method = property.form == kiln_hir::PropertyForm::Method;
                        self.emit_object_segment(&mut pending, accumulating);
                        accumulating = true;
                        match &property.key {
                            ObjectKey::Private(name) => self.error(
                                Some(span),
                                format!("private name `#{name}` in an object literal"),
                            ),
                            ObjectKey::Static(key) => {
                                self.push_constant(Constant::String(key.clone()));
                            }
                            ObjectKey::Computed(key) => self.expression(key),
                        }
                        if method && let Some(home) = &home {
                            self.home_bindings.push(home.clone());
                        }
                        self.next_is_method = method;
                        self.expression(&property.value);
                        if method && home.is_some() {
                            self.home_bindings.pop();
                        }
                        self.code.push(Instruction::DefineNamedProperty);
                        continue;
                    }
                    match &property.key {
                        // Only a class body can declare one, so a literal never has one.
                        ObjectKey::Private(name) => self.error(
                            Some(span),
                            format!("private name `#{name}` in an object literal"),
                        ),
                        ObjectKey::Static(key) => {
                            self.push_constant(Constant::String(key.clone()));
                        }
                        // Evaluated in source order, between the members around it, because a
                        // computed key is an arbitrary expression and may have side effects.
                        ObjectKey::Computed(key) => self.expression(key),
                    }
                    // A method's body is the only part of a member the literal's own home
                    // covers. A value's is not: `{ v: super.m() }` inside a class method reads
                    // that method's home, not this literal's.
                    let method = property.form == kiln_hir::PropertyForm::Method;
                    if method && let Some(home) = &home {
                        self.home_bindings.push(home.clone());
                    }
                    // The body about to be compiled is a method.
                    self.next_is_method = method;
                    self.expression(&property.value);
                    if method && home.is_some() {
                        self.home_bindings.pop();
                    }
                    pending += 1;
                }
                ObjectMember::Spread(source) => {
                    // The first segment becomes the accumulator; later ones merge into it.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    self.expression(source);
                    self.code.push(Instruction::SpreadObject);
                }
                ObjectMember::Accessor {
                    key,
                    kind,
                    function,
                } => {
                    // An accessor is defined on the object rather than pushed as a pair, so the
                    // pending run has to become an object first.
                    self.emit_object_segment(&mut pending, accumulating);
                    accumulating = true;
                    match key {
                        ObjectKey::Private(name) => self.error(
                            Some(span),
                            format!("private name `#{name}` in an object literal"),
                        ),
                        ObjectKey::Static(name) => {
                            self.push_constant(Constant::String(name.clone()));
                        }
                        ObjectKey::Computed(key) => self.expression(key),
                    }
                    if let Some(home) = &home {
                        self.home_bindings.push(home.clone());
                    }
                    self.next_is_method = true;
                    let compiled = self.compile_function(function, span);
                    if home.is_some() {
                        self.home_bindings.pop();
                    }
                    let Some(index) = compiled else {
                        return;
                    };
                    self.code.push(Instruction::MakeFunction(index));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(kind, kiln_hir::MethodKind::Set),
                        enumerable: true,
                    });
                }
            }
        }
        self.emit_object_segment(&mut pending, accumulating);
        // The literal is the home object its methods were defined on, so it is what the binding
        // they captured has to hold -- duplicated rather than reloaded, because the literal is
        // also this expression's value.
        if let Some(home) = &home {
            self.code.push(Instruction::Dup);
            let home = home.clone();
            self.store_home_binding(&home);
        }
    }

    /// Emit the pending run of key/value pairs as an object, merging it into the accumulator
    /// beneath when one exists.
    ///
    /// Skipped when the run is empty and something is already accumulating, so `{ ...a, ...b }`
    /// does not build two throwaway objects.
    fn emit_object_segment(&mut self, pending: &mut u32, merge: bool) {
        if merge && *pending == 0 {
            return;
        }
        self.code.push(Instruction::NewObject { count: *pending });
        *pending = 0;
        if merge {
            self.code.push(Instruction::SpreadObject);
        }
    }

    fn logical(&mut self, operator: LogicalOperator, left: &Expression, right: &Expression) {
        self.expression(left);
        self.code.push(Instruction::Dup);
        let jump = self.emit_jump(match operator {
            LogicalOperator::And => Instruction::JumpIfFalse(0),
            LogicalOperator::Or => Instruction::JumpIfTruthy(0),
            LogicalOperator::NullishCoalesce => Instruction::JumpIfNotNullish(0),
        });
        self.code.push(Instruction::Pop);
        self.expression(right);
        self.patch_jump(jump);
    }

    fn assignment(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let scopes = self.with_scopes_for(target);
        if !scopes.is_empty() {
            self.with_assignment(&scopes, operator, target, value, span);
            return;
        }
        self.assign_lexically(operator, target, value, span);
    }

    /// `name = value` inside a `with`, written to the innermost object that has the name.
    ///
    /// The value expression is compiled once per object plus once for the lexical case. Only one
    /// of those copies ever runs, and the alternative -- spilling the value into a temporary
    /// first -- would evaluate it before the reference is resolved, which is the opposite of the
    /// order the specification gives.
    fn with_assignment(
        &mut self,
        scopes: &[WithScope],
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let key = self.constant_index(Constant::String(target.to_owned()));
        let mut found = Vec::with_capacity(scopes.len());
        for with in scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            if operator.is_short_circuiting() {
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                let skip = self.emit_jump(short_circuit_jump(operator));
                self.load_with_object(with);
                self.expression(value);
                self.code.push(self.set_property_instruction(key));
                let done = self.emit_jump(Instruction::Jump(0));
                self.patch_jump(skip);
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                self.patch_jump(done);
            } else if operator == AssignmentOperator::Assign {
                self.load_with_object(with);
                self.expression(value);
                self.code.push(self.set_property_instruction(key));
            } else {
                self.load_with_object(with);
                self.load_with_object(with);
                self.code.push(Instruction::GetProperty(key));
                self.expression(value);
                self.code
                    .push(Instruction::Binary(compound_operator(operator)));
                self.code.push(self.set_property_instruction(key));
            }
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.assign_lexically(operator, target, value, span);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// `name = value` against the bindings around it, ignoring any `with` object.
    fn assign_lexically(
        &mut self,
        operator: AssignmentOperator,
        target: &str,
        value: &Expression,
        span: Span,
    ) {
        let Some((load, store)) = self.assignment_place(target, span) else {
            return;
        };
        if operator.is_short_circuiting() {
            // `a ??= b` reads `a`, and assigns only if it is the wrong one -- so `b` may never
            // be evaluated and the store may never happen. The current value is left on the
            // stack as the expression's result when the store is skipped.
            self.code.push(load.clone());
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.expression(value);
            self.code.push(Instruction::Dup);
            self.code.push(store);
            let done = self.emit_jump(Instruction::Jump(0));
            self.patch_jump(skip);
            self.code.push(load);
            self.patch_jump(done);
            return;
        }
        if operator == AssignmentOperator::Assign {
            self.expression(value);
        } else {
            self.code.push(load);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        }
        self.code.push(Instruction::Dup);
        self.code.push(store);
    }

    /// Store the value on top of the stack into an existing binding, consuming it.
    ///
    /// The leaf of a destructuring assignment. Unlike [`Compiler::assignment`] there is no
    /// operator and no result to leave behind: the pattern walk already holds what it needs.
    fn store_existing(&mut self, name: &str, span: Span) {
        if let Some((_, store)) = self.assignment_place(name, span) {
            self.code.push(store);
        }
    }

    /// How to read and write `target`, or `None` when it names nothing.
    ///
    /// Reports const and imported-binding assignments, and compiles an unresolved name to a
    /// `ReferenceError` raised where the assignment happens -- so a caller that gets `None` has
    /// already had its diagnostic or its throw emitted and should emit nothing more.
    fn assignment_place(&mut self, target: &str, span: Span) -> Option<(Instruction, Instruction)> {
        // Whether the target is a frame local or a module slot decides the store instruction,
        // so it has to be carried alongside the slot rather than assumed.
        enum Place {
            Local(u32),
            Cell(u32),
            Upvalue(u32),
            Global(u32),
        }
        let (place, kind, imported) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => {
                (Place::Cell(local.slot), local.kind, local.imported)
            }
            Resolution::Local(local) => (Place::Local(local.slot), local.kind, local.imported),
            Resolution::Upvalue(index) => {
                // The owning declaration decides const-ness, and it was checked where the name
                // was captured; an upvalue carries no separate mutability of its own.
                (Place::Upvalue(index), VariableKind::Let, false)
            }
            Resolution::Global(slot) => {
                let module = &self.enclosing[0].scopes[0];
                let local = module
                    .get(target)
                    .copied()
                    .expect("Global resolution just found this name in module scope");
                (Place::Global(slot), local.kind, local.imported)
            }
            // A `var` this program declared on the global object. `ThrowReference` reads it and
            // `StoreGlobalBinding` writes it, exactly as for a name nothing declared -- the
            // difference is that declaration instantiation already created the property, so the
            // read cannot throw and the write cannot be the one that creates it.
            Resolution::GlobalObject => {
                let name = self.constant_index(Constant::String(target.to_owned()));
                return Some((
                    Instruction::ThrowReference(name),
                    Instruction::StoreGlobalBinding(name),
                ));
            }
            Resolution::Unresolved => {
                let _ = span;
                let name = self.constant_index(Constant::String(target.to_owned()));
                // Sloppy code settles an unresolvable reference against the global object, which
                // creates the property rather than refusing the write. Strict code raises the
                // `ReferenceError` where the assignment happens rather than when it is compiled.
                //
                // Only the *store* differs. A compound assignment reads first, and reading a name
                // nothing bound is still a `ReferenceError` in both modes -- `undeclared += 1`
                // throws in sloppy code too -- so the load stays `ThrowReference`, which is also
                // the instruction that finds a global the program installed earlier.
                if self.strict {
                    // Where the reference is *resolved*, which is before the right-hand side is
                    // evaluated. Strict mode refuses to create a global by assignment, and
                    // whether this name is one it would have to create can only be answered at
                    // run time -- a `var` in global code, or a `globalThis.x = 1` a line above,
                    // makes it a perfectly ordinary write.
                    self.code.push(Instruction::RequireGlobalBinding(name));
                }
                return Some((
                    Instruction::ThrowReference(name),
                    if self.strict {
                        Instruction::StoreGlobalBindingStrict(name)
                    } else {
                        Instruction::StoreGlobalBinding(name)
                    },
                ));
            }
        };
        let (load, store): (Instruction, Instruction) = match place {
            Place::Local(slot) => (Instruction::LoadLocal(slot), Instruction::StoreLocal(slot)),
            Place::Cell(slot) => (Instruction::LoadCell(slot), Instruction::StoreCell(slot)),
            Place::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Place::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
        };
        // Assignment is also a read of the binding's state: `x = 1; let x;` is a ReferenceError,
        // not a write that makes the later declaration appear to have run. A switch shares one
        // scope between its cases, so compiler-time `initialized` can be true here after another
        // case's declaration even when this path reaches the assignment first. Probe the
        // run-time slot before every lexical write; initialized bindings pay only a load/pop.
        if kind != VariableKind::Var || matches!(&load, Instruction::LoadUpvalue(_)) {
            self.code.push(load.clone());
            self.code.push(Instruction::Pop);
        }
        if imported {
            self.error(
                Some(span),
                format!("assignment to imported binding `{target}` is not allowed"),
            );
        } else if kind == VariableKind::Const {
            self.error(
                Some(span),
                format!("assignment to const local `{target}` is not supported"),
            );
        }
        Some((load, store))
    }

    /// Compile a class into a constructor and a prototype.
    ///
    /// The desugaring is not a shortcut -- a class really is a constructor function whose
    /// `prototype` holds the instance methods -- so it reuses the machinery `new` already needs
    /// rather than introducing a parallel object model for classes.
    /// A class expression leaves its constructor on the stack.
    fn class_expression(&mut self, class: &Class, span: Span) {
        // A class expression may be anonymous, and its name -- when it has one -- is visible only
        // inside its own body. The scope pushed below is what keeps it there, so a named class
        // expression can bind under its own name: `class A extends B { m() { return A; } }` as an
        // expression has to find `A` from inside `m`, and nowhere else. An anonymous one gets a
        // hidden binding instead, carrying a character an identifier cannot contain.
        let binding = match &class.name {
            Some(name) => name.clone(),
            None => {
                let hidden = format!("%class{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                hidden
            }
        };

        self.scopes.push(HashMap::new());
        let mut named = class.clone();
        named.name = Some(binding.clone());
        self.class_declaration(&named);
        match self.resolve(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            _ => {
                self.error(Some(span), "could not bind class expression");
                self.push_constant(Constant::Undefined);
            }
        }
        self.scopes.pop().expect("class scope was just pushed");
    }

    /// A class body is strict code, whatever encloses it.
    ///
    /// Not only its methods -- which are functions and would inherit it anyway -- but the
    /// heritage expression, the computed keys and the field initialisers, all of which are
    /// compiled into whatever code surrounds the class. So the flag is raised around the whole
    /// declaration and lowered again after it.
    fn class_declaration(&mut self, class: &Class) {
        let outer_strict = std::mem::replace(&mut self.strict, true);
        self.strict_class_declaration(class);
        self.strict = outer_strict;
    }

    fn strict_class_declaration(&mut self, class: &Class) {
        let Some(name) = class.name.clone() else {
            self.error(Some(class.span), "a class declaration requires a name");
            return;
        };
        // Everything compiled from here to the end of this function -- fields, methods, and any
        // class nested inside one -- names its private fields in this class's own scope.
        self.private_scopes.push(self.next_private_scope);
        self.next_private_scope += 1;
        // Declared here so every method compiled below captures it; what it holds is minted
        // once, where the class is evaluated.
        let brand = self.declare_home_binding(class.span);
        self.private_brands.push(brand);
        self.private_declarations.push(
            class
                .initialisers
                .iter()
                .filter_map(ClassInitialiser::as_field)
                .map(|field| &field.key)
                .chain(class.methods.iter().map(|method| &method.key))
                .filter_map(|key| match key {
                    ObjectKey::Private(name) => Some(name.clone()),
                    _ => None,
                })
                .collect(),
        );
        // A derived class evaluates its parent once, into a binding its members capture.
        //
        // The name carries a character an identifier cannot contain, so it can never collide
        // with one the program wrote, and a counter so that two derived classes in one scope get
        // separate bindings. `super` is syntax rather than a variable, but it resolves exactly
        // like a captured one -- giving it a real binding lets it reuse the closure machinery
        // instead of needing a parallel path through every scope.
        if let Some(superclass) = &class.superclass {
            let binding = format!("%super{}", self.super_binding_count);
            self.super_binding_count += 1;
            self.captured.insert(binding.clone());
            self.variable(VariableKind::Const, &binding, Some(superclass), class.span);
            self.super_bindings.push(binding);
        }

        // The two home objects a class has. An instance method, the constructor and every field
        // initialiser are defined on `C.prototype`; a static method and a static field are
        // defined on `C` itself. Declared here, before anything that could capture one is
        // compiled, and filled in below once the constructor exists -- a method closes over the
        // binding, not over what it held at the time.
        let instance_home = self.declare_home_binding(class.span);
        let static_home = self.declare_home_binding(class.span);

        // Bound before the body is compiled, so a method can refer to the class by name.
        self.variable(VariableKind::Let, &name, None, class.span);

        // A class expression with no written name still needs a binding to hold itself, and
        // `class_expression` invents one carrying a `%` that no identifier can contain. That
        // binding is scaffolding: `(class {}).name` is the empty string, and reporting the
        // scaffolding instead put `"%class0"` where the program could read it.
        // A `%` carries the scaffolding binding an anonymous class expression is held in, which
        // the program cannot spell and must never read back. What it *does* read back is the name
        // a named evaluation supplied -- `var C = class {}` is `"C"` -- and the empty string only
        // when nothing named it.
        let written_name = if name.starts_with('%') {
            class.inferred_name.clone().unwrap_or_default()
        } else {
            name.clone()
        };
        let constructor = match &class.constructor {
            // The constructor *is* the class as a value, so it carries the class's name: that is
            // what `new.target.name` and a console's `C {}` both read.
            Some(constructor) => Function {
                name: Some(written_name.clone()),
                // The class's span, not the constructor's. The constructor *is* the class as a
                // value, and `Function.prototype.toString` on a class answers with the whole
                // `class C { ... }` text -- the methods included -- never with the one member
                // that happens to back it.
                span: class.span,
                ..constructor.clone()
            },
            // A class with no constructor gets an empty one, so `new` needs no special case for
            // classes that declared none.
            None => Function {
                rest: None,
                return_type: None,
                asynchronous: false,
                name: Some(written_name),
                // A class binds its own name through `class_expression`, not through this.
                binds_own_name: false,
                arrow: false,
                generator: false,
                parameters: Vec::new(),
                body: Vec::new(),
                strict: true,
                span: class.span,
            },
        };
        let synthesized_derived = class.constructor.is_none() && class.superclass.is_some();
        // A private instance method or accessor is installed on each *instance*, not on the
        // prototype: that is what carries the brand, and what makes `D.f()` on a sibling class a
        // TypeError rather than a call. The kind travels with it, because an accessor read is a
        // call and a method write is a refusal -- neither of which a plain slot can express.
        //
        // Installed *before* the field initialisers rather than after, which is the order the
        // language specifies and an observable one: a field initialiser may call `this.#m()`.
        let private_elements: Vec<(String, PrivateKind, Function)> = class
            .methods
            .iter()
            .filter(|method| !method.is_static)
            .filter_map(|method| {
                let ObjectKey::Private(name) = &method.key else {
                    return None;
                };
                Some((
                    name.clone(),
                    private_kind(method.kind),
                    method.function.clone(),
                ))
            })
            .collect();
        let instance_fields: Vec<&ClassField> = class
            .initialisers
            .iter()
            .filter_map(ClassInitialiser::as_field)
            .filter(|field| !field.is_static)
            .collect();
        self.home_bindings.push(instance_home.clone());
        // Each private method and accessor is created *once*, here, where the class is defined,
        // and kept in a binding the constructor captures. Creating it inside the constructor
        // made a fresh function per instance, so `a.#m === b.#m` was false and a brand check
        // written as an identity comparison failed. It also closed over the constructor's
        // locals rather than the class, which is not the environment the specification gives it.
        // The brand: one fresh object per *evaluation* of this class. An empty array is the
        // cheapest thing with an identity, and nothing ever looks inside it -- only whether an
        // instance's brand is this one.
        if let Some(brand) = self.private_brands.last().cloned() {
            self.code.push(Instruction::NewArray { count: 0 });
            self.store_home_binding(&brand);
        }
        let mut private_bindings: Vec<(String, PrivateKind, String)> =
            Vec::with_capacity(private_elements.len());
        for (name, kind, function) in &private_elements {
            self.next_is_method = true;
            let Some(body) = self.compile_function(function, class.span) else {
                self.home_bindings.pop();
                return;
            };
            let binding = self.declare_home_binding(class.span);
            self.code.push(Instruction::MakeFunction(body));
            self.store_home_binding(&binding);
            private_bindings.push((name.clone(), *kind, binding));
        }
        // A field's *name* is evaluated where the class is defined, once. The binding is
        // declared here so the constructor compiled below captures it; what it holds is computed
        // further down, once the class binding exists to be the receiver `ToPropertyKey` reads.
        let field_bindings: Vec<(&ClassField, Option<String>)> = instance_fields
            .iter()
            .map(|field| {
                let binding = matches!(field.key, ObjectKey::Computed(_))
                    .then(|| self.declare_home_binding(class.span));
                (*field, binding)
            })
            .collect();
        let compiled = self.compile_constructor(
            &constructor,
            class.span,
            Some(ClassBody {
                derived: class.superclass.is_some(),
                // A *derived* class with no constructor is not empty: it forwards its arguments
                // to the parent. Without that, `class F extends Error {}` would construct an
                // error with no message.
                forwards_to_super: synthesized_derived,
                private_elements: &private_bindings,
                fields: &field_bindings,
            }),
        );
        self.home_bindings.pop();
        let Some(index) = compiled else {
            return;
        };

        let Resolution::Local(local) = self.resolve(&name) else {
            self.error(Some(class.span), format!("could not bind class `{name}`"));
            return;
        };
        self.code.push(Instruction::MakeFunction(index));
        self.code.push(if local.celled {
            Instruction::StoreCell(local.slot)
        } else {
            Instruction::StoreLocal(local.slot)
        });

        // The constructor carries the brand too: a static private member lives on it, and the
        // check that reads one has to tell this evaluation's constructor from another's.
        if let Some(brand) = self.private_brands.last().cloned() {
            let key = self.constant_index(Constant::String(self.own_private_brand_key()));
            self.load_local_or_cell(local);
            self.load_captured_binding(&brand);
            self.code.push(self.set_property_instruction(key));
            self.code.push(Instruction::Pop);
        }

        // Now the class binding exists, each computed field name is evaluated and converted --
        // once, here, where the class is defined. A name whose `toString` has an effect has it
        // once rather than once per instance, and one that refuses does so when the class is
        // written rather than when it is first constructed. The class is pushed beneath the key
        // because that is the pair `ToPropertyKey` reads; storing the name consumes it, and the
        // receiver is dropped afterwards.
        for (field, binding) in &field_bindings {
            let (ObjectKey::Computed(key), Some(binding)) = (&field.key, binding) else {
                continue;
            };
            let binding = binding.clone();
            self.load_local_or_cell(local);
            self.expression(key);
            self.code.push(Instruction::ToPropertyKey);
            self.store_home_binding(&binding);
            self.code.push(Instruction::Pop);
        }

        let prototype = self.constant_index(Constant::String("prototype".to_owned()));

        // Link the two prototypes so instances inherit the parent's methods, and the two
        // constructors so static methods are inherited too.
        if class.superclass.is_some() {
            self.load_local_or_cell(local);
            self.code.push(Instruction::GetProperty(prototype));
            self.load_super();
            self.code.push(Instruction::GetProperty(prototype));
            self.code.push(Instruction::SetPrototype);
            self.code.push(Instruction::Pop);

            self.load_local_or_cell(local);
            self.load_super();
            self.code.push(Instruction::SetPrototype);
            self.code.push(Instruction::Pop);
        }

        // Filled after the prototype link, so that reading `C.prototype`'s own prototype from
        // inside a method answers the parent's -- and before the static fields, because a static
        // field initialiser runs *now* and may be the first thing to read one.
        self.load_local_or_cell(local);
        self.store_home_binding(&static_home);
        self.load_local_or_cell(local);
        self.code.push(Instruction::GetProperty(prototype));
        self.store_home_binding(&instance_home);

        // Static fields are ordinary properties of the constructor, assigned once here, and a
        // `static { }` block is a body run once, here, with `this` bound to the constructor.
        // One loop over one list because the two interleave and the order is observable.
        self.home_bindings.push(static_home.clone());
        for initialiser in &class.initialisers {
            let field = match initialiser {
                ClassInitialiser::Field(field) if field.is_static => field,
                ClassInitialiser::Field(_) => continue,
                // Compiled as the function it is and called immediately with the constructor as
                // its receiver -- which is the whole of the block's semantics: `this` is the
                // class, `var` stays inside, and `super.x` reads through the same home binding a
                // static method does.
                ClassInitialiser::StaticBlock(body) => {
                    self.next_is_method = true;
                    let Some(block) = self.compile_function(body, class.span) else {
                        self.home_bindings.pop();
                        return;
                    };
                    self.load_local_or_cell(local);
                    self.code.push(Instruction::MakeFunction(block));
                    self.code.push(Instruction::NewArray { count: 0 });
                    self.code.push(Instruction::Apply);
                    self.code.push(Instruction::Pop);
                    continue;
                }
            };
            self.load_local_or_cell(local);
            match &field.key {
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(self.private_key(name)));
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.guard_static_prototype_key();
                    self.emit_field_value(field, Some(&static_home));
                    self.code.push(self.set_computed_instruction());
                }
            }
            self.code.push(Instruction::Pop);
        }
        self.home_bindings.pop();

        for method in &class.methods {
            self.home_bindings.push(if method.is_static {
                static_home.clone()
            } else {
                instance_home.clone()
            });
            self.next_is_method = true;
            let compiled = self.compile_function(&method.function, class.span);
            self.home_bindings.pop();
            let Some(body) = compiled else {
                return;
            };

            // Static methods hang off the constructor; instance methods off its prototype.
            self.load_local_or_cell(local);
            if !method.is_static {
                self.code.push(Instruction::GetProperty(prototype));
            }

            let accessor = !matches!(method.kind, kiln_hir::MethodKind::Normal);
            match &method.key {
                // Installed on each instance by the constructor prologue instead -- an accessor
                // as much as a method, because both carry the brand and the brand is per object.
                ObjectKey::Private(_) if !method.is_static => {
                    self.code.push(Instruction::Pop);
                    continue;
                }
                // A static private element lives on the constructor, and with its kind: a static
                // private getter read as a slot answered with the getter *function*, which is a
                // wrong answer rather than a missing feature.
                ObjectKey::Private(name) => {
                    let key = self.private_key(name);
                    let name = self.constant_index(Constant::String(key));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefinePrivate {
                        name,
                        kind: private_kind(method.kind),
                    });
                }
                ObjectKey::Static(key) if !accessor => {
                    self.push_constant(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineMethod);
                }
                ObjectKey::Static(key) => {
                    self.push_constant(Constant::String(key.clone()));
                    self.code.push(Instruction::MakeFunction(body));
                    self.code.push(Instruction::DefineAccessor {
                        setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                        enumerable: false,
                    });
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.code.push(Instruction::MakeFunction(body));
                    if accessor {
                        self.code.push(Instruction::DefineAccessor {
                            setter: matches!(method.kind, kiln_hir::MethodKind::Set),
                            enumerable: false,
                        });
                    } else {
                        self.code.push(Instruction::DefineMethod);
                    }
                }
            }
            // Each definition leaves something; nothing consumes it.
            self.code.push(Instruction::Pop);
        }

        if class.superclass.is_some() {
            self.super_bindings.pop();
        }
        self.private_scopes
            .pop()
            .expect("this class pushed its own private scope");
        self.private_brands
            .pop()
            .expect("this class pushed its own brand");
        self.private_declarations
            .pop()
            .expect("this class pushed its own private names");
    }

    /// Push the `[[Prototype]]` of the home object of the method currently being compiled.
    ///
    /// Read at each `super.x` rather than fixed when the method was defined, because that is
    /// where the specification reads it: `Object.setPrototypeOf(C.prototype, X)` changes what
    /// `super` means inside `C`'s methods from that moment on, and a link recorded at class
    /// definition would keep answering the old one.
    fn load_home_prototype(&mut self) {
        let Some(binding) = self.home_bindings.last().cloned() else {
            // Unreachable from any program the frontend accepts -- `super.x` outside a method is
            // a syntax error before it reaches here -- so this reports rather than invents.
            self.error(None, "`super` outside of a method");
            self.push_constant(Constant::Undefined);
            return;
        };
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot));
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, "`super` outside of a method");
                self.push_constant(Constant::Undefined);
                return;
            }
        }
        self.code.push(Instruction::LoadPrototype);
    }

    /// Reserve a hidden binding for a home object, celled so a method body can capture it.
    ///
    /// The value is stored later, by [`Compiler::store_home_binding`], because the object it
    /// names does not exist yet when the methods that capture it are compiled -- a method
    /// closes over the *binding*, and reads it when it runs.
    /// Reserve the hidden binding a derived constructor's `this` lives in, in its dead zone.
    ///
    /// Not `variable`, which would store `undefined` into it and so take it straight back out of
    /// the zone. The cell is created and marked uninitialised and nothing else: the only thing
    /// that fills it is the `super()` that brings the instance into existence.
    fn declare_this_binding(&mut self) -> String {
        let binding = format!("%this{}", self.this_binding_count);
        self.this_binding_count += 1;
        self.captured.insert(binding.clone());
        let slot = self.local_count;
        self.local_count += 1;
        let scope = self.scopes.len() - 1;
        self.scopes[scope].insert(
            binding.clone(),
            Local {
                slot,
                kind: VariableKind::Let,
                imported: false,
                // Initialised as far as *resolution* is concerned: the check is the cell's, at
                // run time, because `super()` may be written in a branch or in an arrow and no
                // compile-time flag can say whether it ran.
                initialized: true,
                celled: true,
            },
        );
        self.code.push(Instruction::NewCell(slot));
        self.code.push(Instruction::MarkUninitialized(slot));
        binding
    }

    /// Push `this` from the hidden binding, raising the dead zone's `ReferenceError` if `super()`
    /// has not run.
    fn load_this_binding(&mut self, binding: &str) {
        match self.resolve_or_capture(binding) {
            Resolution::Local(local) => self.code.push(Instruction::LoadCell(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            _ => {
                self.error(None, "a derived constructor's `this` binding lost its cell");
                self.code.push(Instruction::LoadThis);
            }
        }
    }

    /// Record that `super()` has returned, so `this` may be read from here on.
    ///
    /// A second `super()` therefore does *not* raise -- the binding is simply written again.
    /// That is a narrower fix than the specification's, which makes the second call a
    /// `ReferenceError`, and it is named rather than hidden: what is fixed here is reading `this`
    /// too early, not calling `super()` twice.
    fn store_this_binding(&mut self) {
        let Some(binding) = self.this_binding.clone() else {
            return;
        };
        self.code.push(Instruction::LoadThis);
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) => self.code.push(Instruction::StoreCell(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::StoreUpvalue(index)),
            _ => {
                self.error(None, "a derived constructor's `this` binding lost its cell");
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Push `this`, from wherever this context keeps it.
    fn load_this(&mut self) {
        match self.this_binding.clone() {
            Some(binding) => self.load_this_binding(&binding),
            None => self.code.push(Instruction::LoadThis),
        }
    }

    /// `GetThisBinding()`, for its effect only.
    ///
    /// `super.x` resolves against `this` even though it does not read it, so writing one before
    /// `super()` in a derived constructor is a `ReferenceError` just as writing `this` is.
    fn require_this_binding(&mut self) {
        if self.this_binding.is_some() {
            self.load_this();
            self.code.push(Instruction::Pop);
        }
    }

    fn declare_home_binding(&mut self, span: Span) -> String {
        let binding = format!("%home{}", self.home_binding_count);
        self.home_binding_count += 1;
        self.captured.insert(binding.clone());
        self.variable(VariableKind::Let, &binding, None, span);
        binding
    }

    /// Store the value on top of the stack into a hidden home binding, consuming it.
    fn store_home_binding(&mut self, binding: &str) {
        match self.resolve(binding) {
            Resolution::Local(local) => self.code.push(if local.celled {
                Instruction::StoreCell(local.slot)
            } else {
                Instruction::StoreLocal(local.slot)
            }),
            _ => {
                self.error(None, "a home object binding lost its slot");
                self.code.push(Instruction::Pop);
            }
        }
    }

    /// Push the parent constructor of the class currently being compiled.
    fn load_super(&mut self) {
        let Some(binding) = self.super_bindings.last().cloned() else {
            self.error(None, "`super` outside of a derived class");
            self.push_constant(Constant::Undefined);
            return;
        };
        match self.resolve_or_capture(&binding) {
            Resolution::Local(local) if local.celled => {
                self.code.push(Instruction::LoadCell(local.slot))
            }
            Resolution::Local(local) => self.code.push(Instruction::LoadLocal(local.slot)),
            Resolution::Upvalue(index) => self.code.push(Instruction::LoadUpvalue(index)),
            Resolution::Global(slot) => self.code.push(Instruction::LoadGlobal(slot)),
            Resolution::GlobalObject | Resolution::Unresolved => {
                self.error(None, "`super` outside of a derived class");
                self.push_constant(Constant::Undefined);
            }
        }
    }

    /// A field with no initialiser is `undefined`, not absent: the property exists.
    /// Emit a field's initialiser, in the context the language gives it.
    ///
    /// An initialiser is a function of its own to the specification even though Kiln emits it
    /// inline: `super.x` reads through the class's home object -- which the caller has already
    /// pushed -- and `new.target` is `undefined` rather than whatever constructed the instance.
    /// Both are restored afterwards, because the code around the initialiser is not in one.
    fn emit_field_value(&mut self, field: &ClassField, static_this: Option<&str>) {
        let outer_super_property = std::mem::replace(&mut self.super_property, true);
        let outer_field_initialiser = std::mem::replace(&mut self.field_initialiser, true);
        // An initialiser is its own function to the language, so its `this` is the instance
        // being built rather than any enclosing constructor's dead-zoned binding. An instance
        // field only runs after `super()` anyway; a *static* one is emitted where the class is
        // defined, which may be inside a derived constructor that has not called it yet.
        let outer_this_binding = self.this_binding.take();
        if let Some(binding) = static_this {
            self.this_binding = Some(binding.to_owned());
        }
        match &field.value {
            Some(value) => self.expression(value),
            None => self.push_constant(Constant::Undefined),
        }
        self.super_property = outer_super_property;
        self.field_initialiser = outer_field_initialiser;
        self.this_binding = outer_this_binding;
    }

    fn load_local_or_cell(&mut self, local: Local) {
        self.code.push(if local.celled {
            Instruction::LoadCell(local.slot)
        } else {
            Instruction::LoadLocal(local.slot)
        });
    }

    /// Compile a function body into its own proto and bind the name to a callable.
    ///
    /// Declarations are not hoisted yet: the binding exists from this point on, so calling a
    /// function textually above its declaration is an unresolved identifier rather than a call
    /// to `undefined`. That is a smaller lie than hoisting half-way would be, and it is a
    /// diagnostic rather than a wrong answer.
    /// Compile a function declaration and store it in its already-created binding.
    ///
    /// The binding exists before the body is compiled, which is what makes a function able to
    /// call itself and to be called from above its own text: the body resolves its own name
    /// through the enclosing scope, and the slot it reads is filled by the `MakeFunction` here
    /// before any call can reach it.
    fn emit_function_declaration(&mut self, function: &Function, span: Span) {
        let Some(name) = function.name.clone() else {
            self.error(Some(span), "a function declaration requires a name");
            return;
        };
        // Normally created by the hoisting pass. A function declaration can also appear where a
        // single statement is expected -- `if (x) function f() {}` -- and there is no block to
        // have hoisted it.
        //
        // The test is the *innermost* scope rather than resolution, and the difference is the
        // whole of Annex B B.3.4: an `if` arm is read as though a block were written around it,
        // so `function (f) { if (true) function f() {} }` declares a binding of its own there.
        // Asking `resolve` instead found the enclosing parameter and overwrote it, which is
        // exactly what B.3.2.1 declines to do for a formal.
        let innermost = self.scopes.len() - 1;
        let global_here =
            self.global_var_scope && innermost == 0 && self.global_bindings.contains(&name);
        if !self.scopes[innermost].contains_key(&name) && !global_here {
            self.predeclare(VariableKind::Let, &name, true);
        }

        let Some(index) = self.compile_function(function, span) else {
            return;
        };

        match self.resolve(&name) {
            Resolution::Local(local) => {
                self.code.push(Instruction::MakeFunction(index));
                self.code.push(if local.celled {
                    Instruction::StoreCell(local.slot)
                } else {
                    Instruction::StoreLocal(local.slot)
                });
                self.emit_annexb_var_write(&name, local);
            }
            // A function declaration at the top of global code: the binding is a property of the
            // global object, created by `DeclareGlobalBinding` when the scope was entered and
            // given its value here.
            Resolution::GlobalObject => {
                self.code.push(Instruction::MakeFunction(index));
                self.store_global_object_binding(&name);
            }
            // The binding was just created in the current function's innermost scope, so any
            // other resolution means the scope bookkeeping is inconsistent.
            _ => self.error(Some(span), format!("could not bind function `{name}`")),
        }
    }

    /// Compile one function body, returning its index in [`Program::functions`].
    ///
    /// The body is compiled against a fresh state: its own code buffer, its own slot numbering
    /// starting at the parameters, and its own scope stack. Module scope stays reachable through
    /// `enclosing[0]`; anything between is a capture and is refused by `resolve`.
    fn compile_function(&mut self, function: &Function, span: Span) -> Option<u32> {
        self.compile_constructor(function, span, None)
    }

    /// Install a class's private methods and instance fields on `this`.
    ///
    /// Methods and accessors first, then fields: a field initialiser may call `this.#m()`, and
    /// the language installs the elements that make that call possible before it runs any
    /// initialiser.
    fn emit_instance_elements(
        &mut self,
        private_elements: &[(String, PrivateKind, String)],
        fields: &[(&ClassField, Option<String>)],
        _span: Span,
    ) -> Option<()> {
        // Every instance carries the brand of the evaluation that made it, stored beside its
        // private members and read by each access to tell one evaluation from another.
        if let Some(brand) = self.private_brands.last().cloned() {
            let key = self.constant_index(Constant::String(self.own_private_brand_key()));
            self.code.push(Instruction::LoadThis);
            self.load_captured_binding(&brand);
            self.code.push(self.set_property_instruction(key));
            self.code.push(Instruction::Pop);
        }
        for (name, kind, binding) in private_elements {
            let key = self.private_key(name);
            let name = self.constant_index(Constant::String(key));
            // The class made this function once; every instance is given *that* one, which is
            // what makes `a.#m === b.#m` true.
            self.code.push(Instruction::LoadThis);
            self.load_captured_binding(binding);
            self.code
                .push(Instruction::DefinePrivate { name, kind: *kind });
            self.code.push(Instruction::Pop);
        }
        for (field, name_binding) in fields {
            self.code.push(Instruction::LoadThis);
            if let Some(binding) = name_binding {
                // The name was computed and converted once, where the class is defined.
                self.load_captured_binding(binding);
                self.emit_field_value(field, None);
                self.code.push(self.set_computed_instruction());
                self.code.push(Instruction::Pop);
                continue;
            }
            match &field.key {
                ObjectKey::Private(name) => {
                    let name = self.constant_index(Constant::String(self.private_key(name)));
                    self.emit_field_value(field, None);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Static(key) => {
                    let name = self.constant_index(Constant::String(key.clone()));
                    self.emit_field_value(field, None);
                    self.code.push(self.set_property_instruction(name));
                }
                ObjectKey::Computed(key) => {
                    self.expression(key);
                    self.emit_field_value(field, None);
                    self.code.push(self.set_computed_instruction());
                }
            }
            self.code.push(Instruction::Pop);
        }
        Some(())
    }

    /// The elements a derived constructor owes its instance, emitted where `super()` just
    /// returned.
    ///
    /// The `super()` expression has left the parent's answer on the stack and every sequence
    /// below is stack-balanced, so the expression's own value survives being followed by this.
    fn emit_elements_after_super(&mut self, span: Span) {
        let Some(Some(elements)) = self.pending_elements.last().cloned() else {
            return;
        };
        let fields: Vec<(&ClassField, Option<String>)> = elements
            .fields
            .iter()
            .map(|(field, binding)| (field, binding.clone()))
            .collect();
        self.emit_instance_elements(&elements.private, &fields, span);
    }

    /// Compile a constructor, whose prologue initialises the class's instance fields.
    ///
    /// Fields run after `super()` and before the body, which is the order the language specifies
    /// and the only one that works: a field initialiser may read `this`, and in a derived class
    /// `this` does not exist until the parent has run.
    fn compile_constructor(
        &mut self,
        function: &Function,
        span: Span,
        class: Option<ClassBody<'_>>,
    ) -> Option<u32> {
        if function.parameters.len() > u32::MAX as usize {
            self.error(Some(span), "too many parameters");
            return None;
        }
        if self.functions.len() >= u32::MAX as usize {
            self.error(Some(span), "too many functions in one module");
            return None;
        }

        // Which of this function's own bindings its nested functions refer to. Computed before
        // the body is compiled because a declaration has to know, at the point it allocates its
        // slot, whether that slot holds a value or a cell.
        let mut captured = names_captured_by_this_function(function);
        // `super` is syntax, not an identifier, so the ordinary name walk cannot discover the
        // hidden binding that represents it. Every callable compiled while a derived class is
        // active may emit `load_super` (an explicit call/member access, or a synthesized
        // forwarding constructor) and must carry that binding with the closure. Otherwise an
        // exported derived class reads the caller module's slot by number when constructed.
        if let Some(super_binding) = self.super_bindings.last() {
            captured.insert(super_binding.clone());
        }
        // A `with` object is a hidden binding for the same reason, and the name walk cannot see
        // it either. A function written inside a `with` body still resolves its names against
        // that object when it is called, which may be long after the statement has ended -- so
        // the closure has to carry it.
        for with in &self.with_scopes {
            captured.insert(with.name.clone());
        }
        // A direct `eval` anywhere in this function -- including inside one nested in it -- means
        // the eval'd source may name any binding here, so every one of them has to be a cell.
        let cell_everything = function_contains_direct_eval(function);

        // Every parameter owns a positional slot, whether or not it binds a name there. A plain
        // identifier binds the slot itself, which keeps the common call free of any prologue; a
        // pattern leaves the slot anonymous and is destructured out of it below, into ordinary
        // locals that behave like any other binding.
        let mut parameters = HashMap::with_capacity(function.parameters.len());
        let mut celled_parameters = Vec::new();
        let mut binding_steps = Vec::new();
        // Whether this list has a dead zone to keep at all. Without a default there is nothing
        // that can run while a parameter is unbound, so a function with none pays nothing and
        // behaves exactly as it did.
        let parameters_have_defaults = function
            .parameters
            .iter()
            .any(|parameter| matches!(parameter.pattern, Pattern::WithDefault { .. }));
        for (slot, parameter) in function.parameters.iter().enumerate() {
            let slot = slot as u32;
            let identifier = match &parameter.pattern {
                Pattern::Identifier(name) => Some(name),
                Pattern::WithDefault { inner, .. } => match inner.as_ref() {
                    Pattern::Identifier(name) => Some(name),
                    _ => None,
                },
                _ => None,
            };
            let Some(name) = identifier else {
                binding_steps.push(ParameterStep::Destructure(
                    slot,
                    &parameter.pattern,
                    parameter.span,
                ));
                continue;
            };
            if parameters.contains_key(name) {
                self.error(
                    Some(parameter.span),
                    format!("duplicate parameter `{name}`"),
                );
                return None;
            }
            let celled = captured.contains(name) || cell_everything;
            if celled {
                celled_parameters.push(slot);
            }
            parameters.insert(
                name.clone(),
                Local {
                    slot,
                    // Parameters share a function's variable environment with `var`.
                    // Their initialization rules are still handled below, but `var x` in a
                    // body whose parameter is `x` must reuse this slot.
                    kind: VariableKind::Var,
                    imported: false,
                    // Uninitialised until the binding list reaches it, which is what puts the
                    // parameters to a default's right in a dead zone.
                    initialized: !parameters_have_defaults,
                    celled,
                },
            );
            binding_steps.push(
                if matches!(parameter.pattern, Pattern::WithDefault { .. }) {
                    ParameterStep::Defaulted(slot, celled, &parameter.pattern, parameter.span, name)
                } else {
                    ParameterStep::Plain(name)
                },
            );
        }

        let parameter_count = function.parameters.len() as u32;
        // Annex B declines to give a block-level function declaration a var-scoped binding when
        // the name is a formal parameter or `arguments`: the call already owns that binding, and
        // the block's function must not take it over. `arguments` is here even when the function
        // never mentions it, because the object exists whether or not it is named.
        let mut parameter_names: HashSet<String> = parameters.keys().cloned().collect();
        if let Some(rest) = &function.rest {
            let mut rest_names = Vec::new();
            pattern_names_in_order(rest, &mut rest_names);
            parameter_names.extend(rest_names);
        }
        if !function.arrow {
            parameter_names.insert("arguments".to_owned());
        }
        // A rest parameter is already excluded: it lives in `function.rest` rather than among
        // the positional ones, so counting stops at the first default on its own.
        let declared_length = function
            .parameters
            .iter()
            .take_while(|parameter| !matches!(parameter.pattern, Pattern::WithDefault { .. }))
            .count() as u32;
        let outer_finalizer_depth =
            std::mem::replace(&mut self.function_finalizer_depth, self.finalizers.len());
        // A nested body cannot jump out to an enclosing loop, so it starts with no handlers of
        // its own to account for.
        let outer_handler_depth = std::mem::replace(&mut self.handler_depth, 0);
        // An arrow cannot contain a `yield`, so it never reads this; every other function body
        // establishes its own generator kind, and restores the enclosing one on the way out.
        let outer_async_generator = std::mem::replace(
            &mut self.async_generator,
            function.generator && function.asynchronous,
        );
        // Strictness is inherited and never given up: a body written inside strict code is
        // strict whether or not it says so, and a `"use strict"` of its own turns it on for
        // this body and everything nested in it. Restored on the way out, because a sibling
        // written after a strict function is not strict because of it.
        let inherited_strict = self.strict || function.strict;
        let outer_strict = std::mem::replace(&mut self.strict, inherited_strict);
        // `IsSimpleParameterList`, and the strictness beside it: together they are the
        // specification's own test for which of the two `arguments` objects this function gets.
        //
        // An arrow inherits rather than deciding, because the `arguments` it sees is not its
        // own: it is the enclosing call's, and so is the kind.
        let mapped_arguments = if function.arrow {
            self.mapped_arguments
        } else {
            !inherited_strict
                && function.rest.is_none()
                && function
                    .parameters
                    .iter()
                    .all(|parameter| matches!(parameter.pattern, Pattern::Identifier(_)))
        };
        let outer_mapped_arguments =
            std::mem::replace(&mut self.mapped_arguments, mapped_arguments);
        // An arrow has no boundary of its own for any of these: `super.x`, `new.target` and a
        // field initialiser's `undefined` new.target all reach through it to where it was
        // written. Every other function is a boundary, and `next_is_method` -- set by whichever
        // caller is compiling a method body -- is what says whether this one was written as a method.
        // It is taken either way, so it cannot leak into the next function compiled.
        let method = std::mem::take(&mut self.next_is_method);
        let is_method = method && !function.arrow && class.is_none();
        let outer_in_function = self.in_function;
        let outer_super_property = self.super_property;
        let outer_field_initialiser = self.field_initialiser;
        let outer_this_binding = self.this_binding.take();
        let outer_derived_constructor = std::mem::replace(
            &mut self.derived_constructor,
            class.as_ref().is_some_and(|class| class.derived),
        );
        if !function.arrow {
            self.in_function = true;
            self.super_property =
                (method && self.home_bindings.last().is_some()) || class.is_some();
            self.field_initialiser = false;
        } else {
            // An arrow has no `this` of its own, so it reads the enclosing one -- including a
            // derived constructor's, which is what makes `constructor() { const go = () =>
            // super(); go(); }` legal and its `this` still dead until `go` runs.
            self.this_binding.clone_from(&outer_this_binding);
        }
        // A function body has a variable environment of its own, whatever encloses it: `var i`
        // inside a function written at the top of a script is that call's binding and not a
        // property of the global object. `global_bindings` stays, because the names the *program*
        // put on the object are still resolvable from in here -- it is only new declarations that
        // stop going there.
        let outer_global_var_scope = std::mem::replace(&mut self.global_var_scope, false);
        let outer = FunctionState {
            code: std::mem::take(&mut self.code),
            scopes: std::mem::replace(&mut self.scopes, vec![parameters]),
            local_count: std::mem::replace(&mut self.local_count, parameter_count),
            upvalues: std::mem::take(&mut self.upvalues),
            upvalue_names: std::mem::take(&mut self.upvalue_names),
            captured: std::mem::replace(&mut self.captured, captured),
            cell_everything: std::mem::replace(&mut self.cell_everything, cell_everything),
        };
        self.enclosing.push(outer);

        // A class constructor is not callable. `[[Call]]` on one throws before it looks at an
        // argument, which is why this is the first thing in the body rather than part of the
        // parameter prologue: `Derived.apply(obj, new Array(100))` must throw without binding a
        // hundred parameters, and `C()` on a class whose constructor has a default must throw
        // without running it.
        //
        // Emitted as ordinary bytecode rather than a flag on the proto, because `new.target` is
        // exactly the question being asked and every backend already answers it -- so the C
        // backend gets the same refusal from the same instructions rather than a second
        // implementation that could disagree with this one.
        if class.is_some() {
            self.code.push(Instruction::LoadNewTarget);
            let constructed = self.emit_jump(Instruction::JumpIfNotNullish(0));
            self.code.push(Instruction::LoadBuiltin(Builtin::TypeError));
            self.push_constant(Constant::String(match &function.name {
                Some(name) if !name.is_empty() => {
                    format!("Class constructor {name} cannot be invoked without 'new'")
                }
                _ => "Class constructor cannot be invoked without 'new'".to_owned(),
            }));
            self.code.push(Instruction::New { argc: 1 });
            self.code.push(Instruction::Throw);
            self.patch_jump(constructed);
        }

        // A derived constructor's `this` starts in a dead zone that only `super()` ends.
        if class.as_ref().is_some_and(|class| class.derived) {
            self.this_binding = Some(self.declare_this_binding());
        }

        // A captured parameter arrives as a plain value in its slot, so the cell has to be
        // created and seeded before the body can read it.
        for slot in celled_parameters {
            self.code.push(Instruction::LoadLocal(slot));
            self.code.push(Instruction::NewCell(slot));
            self.code.push(Instruction::StoreCell(slot));
        }

        // Bind the parameters in declaration order, before anything in the body can read them.
        // In *declaration* order rather than patterns-then-defaults, because the order is
        // observable: a default is an expression, and it runs after everything to its left has
        // been bound and before anything to its right has. `bind_pattern` is the same code a
        // `const { a } = x` uses, so defaults, renaming, nesting and rest behave identically
        // wherever they are written.
        let outer_dead_zone =
            std::mem::replace(&mut self.parameter_dead_zone, parameters_have_defaults);
        // A direct `eval` written in one of these defaults declares into the *body's* variable
        // environment, with this parameter record sitting between the two -- so a `var` naming
        // anything the record holds is an early error. Only a list with defaults has a record of
        // its own; without one there is nowhere for an `eval` to be written anyway.
        //
        // `parameter_names` is the set the record holds: the formals, the rest pattern's names,
        // and `arguments` for anything but an arrow. Sorted, because the bytecode is a cache key.
        let outer_parameter_scope = std::mem::replace(
            &mut self.parameter_scope,
            parameters_have_defaults.then(|| {
                let mut names: Vec<String> = parameter_names.iter().cloned().collect();
                names.sort();
                names
            }),
        );
        for step in binding_steps {
            match step {
                ParameterStep::Destructure(slot, pattern, span) => {
                    self.code.push(Instruction::LoadLocal(slot));
                    self.bind_pattern(VariableKind::Let, pattern, span);
                }
                ParameterStep::Defaulted(slot, celled, pattern, span, name) => {
                    self.code.push(if celled {
                        Instruction::LoadCell(slot)
                    } else {
                        Instruction::LoadLocal(slot)
                    });
                    self.assign_pattern(pattern, span);
                    // Past its own default, and so out of the dead zone for everything after it.
                    self.initialize(name);
                }
                // Already in its slot: the call put it there, and there is nothing to emit.
                ParameterStep::Plain(name) => self.initialize(name),
            }
        }
        self.parameter_dead_zone = outer_dead_zone;
        // The body's variable environment is the one an `eval` written there declares into, so
        // there is nothing between the two and nothing to block. Restored on the way out.
        self.parameter_scope = None;

        // `...rest` is the arguments beyond the declared parameters. Taken from the call rather
        // than from slots, because its length is decided by the caller.
        if let Some(rest) = &function.rest {
            self.code.push(Instruction::RestArguments(parameter_count));
            self.bind_pattern(VariableKind::Let, rest, function.span);
        }

        // Everything above binds parameters, and so is what the language calls
        // FunctionDeclarationInstantiation; everything below is the body. A generator has to be
        // able to run the first without the second, so the seam is recorded rather than left
        // implicit in the instruction stream.
        let prologue_end = self.code.len() as u32;

        // Every `var` in the body belongs to *this* function, so its slot and its `undefined`
        // are established here rather than wherever the block containing it happens to be
        // compiled.
        self.hoist_var_names(&function.body);
        // After the `var`s, because Annex B only adds a binding a `var` did not already make --
        // and after the parameters, whose names it must not take over.
        self.hoist_annexb_function_names(&function.body, &parameter_names);
        if let Some(class) = &class {
            if class.forwards_to_super {
                // `super(...arguments)`, and nothing else can run before it -- so by the line
                // after, the instance is already the parent's and the elements go in there.
                self.load_super();
                self.code.push(Instruction::SuperForward);
                self.code.push(Instruction::Pop);
                self.store_this_binding();
            }
            // A derived class with a constructor of its own installs its elements when *that*
            // constructor's `super()` returns, which is a place only the body knows. Anywhere
            // else -- a base class, or the forward just emitted -- `this` is already the
            // instance, so here is the place.
            if class.installs_elements_at_super() {
                self.pending_elements.push(Some(class.elements()));
            } else {
                self.emit_instance_elements(class.private_elements, class.fields, span)?;
                // Pushed even with nothing owed: that is what stops a `super()` in a *nested*
                // class's constructor from reaching past it to this one's elements.
                self.pending_elements.push(None);
            }
        }
        self.statements(&function.body);
        // An ordinary function never took an entry, because an arrow is where a derived
        // constructor's `super()` is allowed to be written -- `constructor() { const go = () =>
        // super(); go(); }` is legal -- and an entry there would hide the constructor's own
        // elements from it.
        if class.is_some() {
            self.pending_elements.pop();
        }
        // Falling off the end of a function returns undefined -- except a derived constructor,
        // which returns `this`, and so raises the dead zone's ReferenceError when it reaches
        // here without having called `super()`. Emitted unconditionally rather than only when
        // the body cannot fall through: proving that needs flow analysis, and an unreachable
        // epilogue costs two instructions.
        if self.derived_constructor {
            let binding = self
                .this_binding
                .clone()
                .expect("a derived constructor has a hidden `this` binding");
            self.load_this_binding(&binding);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.code.push(Instruction::Return);

        let restored = self
            .enclosing
            .pop()
            .expect("this function pushed its own enclosing state");
        self.global_var_scope = outer_global_var_scope;
        let strict = std::mem::replace(&mut self.strict, outer_strict);
        self.mapped_arguments = outer_mapped_arguments;
        self.in_function = outer_in_function;
        self.super_property = outer_super_property;
        self.field_initialiser = outer_field_initialiser;
        self.this_binding = outer_this_binding;
        self.derived_constructor = outer_derived_constructor;
        let parameter_types: Vec<Option<TypeAnnotation>> = function
            .parameters
            .iter()
            .map(|p| p.type_annotation)
            .collect();
        let type_annotations =
            if parameter_types.iter().any(|p| p.is_some()) || function.return_type.is_some() {
                Some(crate::TypeAnnotations {
                    parameters: parameter_types,
                    return_type: function.return_type,
                })
            } else {
                None
            };
        let proto = FunctionProto {
            name: function.name.clone(),
            arrow: function.arrow,
            asynchronous: function.asynchronous,
            // An async body suspends at every `await`, which is a `yield` in every way the
            // machine cares about -- so it is compiled as a generator and driven by the async
            // driver rather than by the program.
            generator: function.generator || function.asynchronous,
            async_generator: function.generator && function.asynchronous,
            is_method,
            parameter_count,
            mapped_arguments,
            declared_length,
            local_count: std::mem::replace(&mut self.local_count, restored.local_count),
            prologue_end,
            code: std::mem::replace(&mut self.code, restored.code),
            upvalues: std::mem::replace(&mut self.upvalues, restored.upvalues),
            span: function.span,
            type_annotations,
        };
        self.scopes = restored.scopes;
        self.function_finalizer_depth = outer_finalizer_depth;
        self.handler_depth = outer_handler_depth;
        self.async_generator = outer_async_generator;
        self.upvalue_names = restored.upvalue_names;
        self.captured = restored.captured;
        self.cell_everything = restored.cell_everything;
        self.parameter_scope = outer_parameter_scope;

        let index = self.functions.len() as u32;
        self.functions.push(proto);
        self.strict_functions.push(strict);
        Some(index)
    }

    fn return_statement(&mut self, value: Option<&Expression>, span: Span) {
        if !self.compiling_function() {
            self.error(Some(span), "`return` outside of a function");
            return;
        }
        // `return;` in a derived constructor yields `this`, and reading it is what makes
        // returning before `super()` a ReferenceError. But the read happens where `[[Construct]]`
        // does it -- *after* the cleanups -- because a `finally` is allowed to be where `super()`
        // is called: `constructor() { try { return; } finally { super(); } }` constructs, and
        // reading `this` at the `return` would have refused it.
        let deferred_this = value.is_none() && self.derived_constructor;
        match value {
            Some(value) => self.expression(value),
            None if deferred_this => {}
            // Anywhere else a bare `return` is `undefined`, and reaching for `this` there would
            // have made every one of them answer with the receiver.
            None => self.push_constant(Constant::Undefined),
        }
        if !self.finalizers.is_empty() {
            if deferred_this {
                self.unwind_finalizers(self.function_finalizer_depth);
            } else {
                // The value is computed before the cleanups run -- that is the observable order
                // -- but the cleanups may use the stack, so it is parked in a local across them.
                let slot = format!("%ret{}", self.hidden_bindings);
                self.hidden_bindings += 1;
                self.scopes.push(HashMap::new());
                self.declare_bound(VariableKind::Let, &slot, span);
                self.unwind_finalizers(self.function_finalizer_depth);
                self.load_hidden(&slot);
                self.scopes.pop().expect("return scope was just pushed");
            }
        }
        if deferred_this {
            let binding = self
                .this_binding
                .clone()
                .expect("a deferred `this` has a binding to read");
            self.load_this_binding(&binding);
        } else if self.derived_constructor {
            // `return v` in a derived constructor, where the value is not simply handed back.
            let binding = self
                .this_binding
                .clone()
                .expect("a derived constructor has a hidden `this` binding");
            self.guard_derived_return(&binding);
        }
        self.code.push(Instruction::Return);
    }

    /// `object.property = value`, and the compound forms.
    ///
    /// The object expression is evaluated once. `o().x += 1` must not call `o` twice, so the
    /// reference is duplicated on the stack rather than the expression being compiled twice --
    /// which would be the easy mistake and would double any side effect in the receiver.
    /// `x++` and friends.
    ///
    /// The two forms differ only in when the value is duplicated: a prefix update yields the new
    /// value, a postfix one the old. That is the whole reason this is not lowered to `x = x + 1`.
    fn update(&mut self, increment: bool, prefix: bool, target: &str, span: Span) {
        let scopes = self.with_scopes_for(target);
        if !scopes.is_empty() {
            self.with_update(&scopes, increment, prefix, target, span);
            return;
        }
        self.update_lexically(increment, prefix, target, span);
    }

    /// `name++` inside a `with`, updating the innermost object that has the name.
    ///
    /// The old value is spilled into a hidden binding rather than juggled on the stack: the
    /// object has to sit beneath the new value for the store, and the old one has to survive
    /// past it for a postfix update, which is one value too many to keep in order otherwise.
    fn with_update(
        &mut self,
        scopes: &[WithScope],
        increment: bool,
        prefix: bool,
        target: &str,
        span: Span,
    ) {
        let key = self.constant_index(Constant::String(target.to_owned()));
        let held = format!("%update{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        let mut found = Vec::with_capacity(scopes.len());
        for with in scopes {
            self.load_with_object(with);
            self.code.push(Instruction::WithHas(key));
            let miss = self.emit_jump(Instruction::JumpIfFalse(0));
            self.load_with_object(with);
            self.code.push(Instruction::GetProperty(key));
            // `ToNumeric` first, for the same reason the lexical form does it: `x++` where `x`
            // is "5" is 5 then 6, not "5" then "51".
            self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
            self.declare_hidden(&held, span);
            self.load_with_object(with);
            self.load_hidden(&held);
            self.code.push(Instruction::Unary(if increment {
                UnaryOp::Increment
            } else {
                UnaryOp::Decrement
            }));
            self.code.push(self.set_property_instruction(key));
            if !prefix {
                self.code.push(Instruction::Pop);
                self.load_hidden(&held);
            }
            found.push(self.emit_jump(Instruction::Jump(0)));
            self.patch_jump(miss);
        }
        self.update_lexically(increment, prefix, target, span);
        for jump in found {
            self.patch_jump(jump);
        }
    }

    /// `name++` against the bindings around it, ignoring any `with` object.
    fn update_lexically(&mut self, increment: bool, prefix: bool, target: &str, span: Span) {
        let (load, store) = match self.resolve_or_capture(target) {
            Resolution::Local(local) if local.celled => (
                Instruction::LoadCell(local.slot),
                Instruction::StoreCell(local.slot),
            ),
            Resolution::Local(local) => (
                Instruction::LoadLocal(local.slot),
                Instruction::StoreLocal(local.slot),
            ),
            Resolution::Upvalue(index) => (
                Instruction::LoadUpvalue(index),
                Instruction::StoreUpvalue(index),
            ),
            Resolution::Global(slot) => (
                Instruction::LoadGlobal(slot),
                Instruction::StoreGlobal(slot),
            ),
            Resolution::GlobalObject => {
                let name = self.constant_index(Constant::String(target.to_owned()));
                (
                    Instruction::ThrowReference(name),
                    Instruction::StoreGlobalBinding(name),
                )
            }
            // `x++` on a name nothing bound is settled against the global object, exactly as a
            // read and a write of it separately would be. The read comes first and is
            // `ThrowReference`, so an unresolvable name is the `ReferenceError` the language
            // promises without the compiler having to decide it -- which it cannot: a global the
            // program installed earlier, or one global code declared with `var`, is resolvable.
            Resolution::Unresolved => {
                let _ = span;
                let name = self.constant_index(Constant::String(target.to_owned()));
                (
                    Instruction::ThrowReference(name),
                    if self.strict {
                        Instruction::StoreGlobalBindingStrict(name)
                    } else {
                        Instruction::StoreGlobalBinding(name)
                    },
                )
            }
        };

        self.code.push(load);
        // `ToNumeric` first: `x++` where `x` is "5" is 5 then 6, not "5" then "51". Without a
        // conversion the `+ 1` below concatenates, which is the trap the member form fell into --
        // and `ToNumber` is not the conversion an update applies: it narrows a BigInt, so `1n++`
        // left a *number* behind and the addition would have refused to mix the two anyway.
        self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
        if !prefix {
            // Keep the old value beneath the arithmetic; the store below consumes the new one.
            self.code.push(Instruction::Dup);
        }
        self.code.push(Instruction::Unary(if increment {
            UnaryOp::Increment
        } else {
            UnaryOp::Decrement
        }));
        if prefix {
            self.code.push(Instruction::Dup);
        }
        self.code.push(store);
    }

    /// An array literal, which may contain spreads.
    ///
    /// Without a spread this is one `NewArray` over the elements. With one, the array is built
    /// incrementally: the run of literal elements before each spread becomes its own `NewArray`
    /// and the spread appends to it, because a spread's length is not known until it runs.
    fn array_literal(&mut self, elements: &[ListElement], span: Span) {
        if elements.len() > u32::MAX as usize {
            self.error(Some(span), "array literal has too many elements");
            return;
        }

        if !elements
            .iter()
            .any(|element| matches!(element, ListElement::Spread(_)))
        {
            for element in elements {
                match element {
                    ListElement::Item(value) => self.expression(value),
                    // A hole occupies its position without contributing a value, which is what
                    // makes it different from an element that happens to be undefined.
                    ListElement::Hole => self.code.push(Instruction::PushHole),
                    ListElement::Spread(_) => {
                        unreachable!("just checked that there are no spreads")
                    }
                }
            }
            self.code.push(Instruction::NewArray {
                count: elements.len() as u32,
            });
            return;
        }

        let mut pending = 0u32;
        let mut started = false;
        for element in elements {
            match element {
                ListElement::Item(value) => {
                    self.expression(value);
                    pending += 1;
                }
                ListElement::Hole => {
                    self.code.push(Instruction::PushHole);
                    pending += 1;
                }
                ListElement::Spread(value) => {
                    self.flush_array_run(&mut pending, &mut started);
                    self.expression(value);
                    self.code.push(Instruction::SpreadInto);
                }
            }
        }
        self.flush_array_run(&mut pending, &mut started);
    }

    /// Turn the pending literal elements into an array, or append them to the one being built.
    fn flush_array_run(&mut self, pending: &mut u32, started: &mut bool) {
        if !*started {
            self.code.push(Instruction::NewArray { count: *pending });
            *started = true;
            *pending = 0;
            return;
        }
        if *pending > 0 {
            self.code.push(Instruction::NewArray { count: *pending });
            self.code.push(Instruction::SpreadInto);
            *pending = 0;
        }
    }

    /// `object[key] = value`, and the compound forms.
    ///
    /// Both the object and the key are evaluated once, in that order, which is the order the
    /// language specifies and the one a reader assumes. A compound form has to re-read through
    /// the same pair rather than re-evaluating either, so both are duplicated on the stack.
    ///
    /// Evaluating the key once is not enough on its own: `ToPropertyKey` is the visible half,
    /// and leaving it to `GetComputed` and `SetComputed` ran it twice. The compound form converts
    /// once up front, which is what the specification's Reference Record memoises. The plain form
    /// must not: there the key converts in `PutValue`, after the right-hand side has run.
    fn computed_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &Expression,
        value: &Expression,
    ) {
        if operator.is_short_circuiting() {
            self.short_circuiting_member(operator, object, None, Some(property), value);
            return;
        }
        self.expression(object);
        self.expression(property);

        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::ToPropertyKey);
            self.code.push(Instruction::Dup2);
            self.code.push(Instruction::GetComputed);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }

        self.code.push(self.set_computed_instruction());
    }

    /// `object.property ??= value`, and the `&&=`/`||=` forms, static or computed.
    ///
    /// These read the property and assign only if it is the wrong one, so the right-hand side may
    /// never be evaluated and the store may never happen -- and the receiver, the key and the
    /// current value each have to survive that decision without being computed twice. Hidden
    /// locals hold them, for the same reason an update does.
    fn short_circuiting_member(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: Option<u32>,
        key: Option<&Expression>,
        value: &Expression,
    ) {
        let span = object.span;
        let receiver = format!("%sc{}", self.hidden_bindings);
        let key_binding = format!("%sckey{}", self.hidden_bindings);
        let current = format!("%sccur{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);
        if let Some(key) = key {
            self.expression(key);
            self.declare_hidden(&key_binding, span);
        }

        self.load_hidden(&receiver);
        match name {
            Some(name) => self.code.push(Instruction::GetProperty(name)),
            None => {
                // The key converts once and the binding keeps the converted one, so the write
                // below reads the same key the read did rather than running `toString` again.
                self.load_hidden(&key_binding);
                self.code.push(Instruction::ToPropertyKey);
                self.code.push(Instruction::Dup);
                self.store_hidden(&key_binding);
                self.code.push(Instruction::GetComputed);
            }
        }
        self.declare_hidden(&current, span);

        self.load_hidden(&current);
        let skip = self.emit_jump(short_circuit_jump(operator));

        self.load_hidden(&receiver);
        if name.is_none() {
            self.load_hidden(&key_binding);
        }
        self.expression(value);
        match name {
            Some(name) => self.code.push(self.set_property_instruction(name)),
            None => self.code.push(self.set_computed_instruction()),
        }
        let done = self.emit_jump(Instruction::Jump(0));

        // Skipped: the expression evaluates to the value that was already there.
        self.patch_jump(skip);
        self.load_hidden(&current);
        self.patch_jump(done);
        self.scopes.pop().expect("assignment scope was just pushed");
    }

    /// `object.property++`, and the other three spellings.
    ///
    /// The object and the key are each evaluated once and held in hidden locals: `o().x++` must
    /// not call `o` twice, and `a[i()]++` must not call `i` twice. That is the same rule a
    /// compound member assignment follows, and the reason neither can be lowered to
    /// `o.x = o.x + 1`.
    fn update_member(
        &mut self,
        increment: bool,
        prefix: bool,
        object: &Expression,
        property: &ObjectKey,
        span: Span,
    ) {
        // A private name has no computed half and no prototype to fall through to, so the general
        // path below -- which exists to evaluate an object and a key exactly once each -- reduces
        // to holding the receiver. Refusing it outright made `this.#n++` a compile error, which
        // is the ordinary way a private counter is incremented.
        if let ObjectKey::Private(name) = property {
            let brand = self.private_brand(name);
            let key = self.private_key(name);
            let name = self.constant_index(Constant::String(key));
            let receiver = format!("%upd{}", self.hidden_bindings);
            let old_binding = format!("%old{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.expression(object);
            self.declare_hidden(&receiver, span);

            // `ToNumeric` for the same reason the general path uses it: the value a postfix
            // update yields is the conversion's result, and a BigInt must not narrow.
            self.load_hidden(&receiver);
            self.push_private_brand(&brand);
            self.code.push(Instruction::GetPrivate(name));
            self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
            self.declare_hidden(&old_binding, span);

            self.load_hidden(&receiver);
            self.load_hidden(&old_binding);
            self.code.push(Instruction::Unary(if increment {
                UnaryOp::Increment
            } else {
                UnaryOp::Decrement
            }));
            self.push_private_brand(&brand);
            self.code.push(Instruction::SetPrivate(name));
            if !prefix {
                self.code.push(Instruction::Pop);
                self.load_hidden(&old_binding);
            }
            self.scopes.pop().expect("update scope was just pushed");
            return;
        }

        let receiver = format!("%upd{}", self.hidden_bindings);
        let key_binding = format!("%updkey{}", self.hidden_bindings);
        let old_binding = format!("%old{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);
        let name = match property {
            ObjectKey::Static(name) => Some(self.constant_index(Constant::String(name.clone()))),
            ObjectKey::Computed(key) => {
                self.expression(key);
                self.declare_hidden(&key_binding, span);
                None
            }
            ObjectKey::Private(_) => unreachable!("refused above"),
        };

        // Read the current value, as a *numeric*: the old value a postfix update yields is the
        // conversion's result rather than whatever was stored.
        self.load_hidden(&receiver);
        match name {
            Some(name) => self.code.push(Instruction::GetProperty(name)),
            None => {
                // The key converts once and the binding keeps the converted one, so the write
                // below reads the same key the read did rather than running `toString` again.
                self.load_hidden(&key_binding);
                self.code.push(Instruction::ToPropertyKey);
                self.code.push(Instruction::Dup);
                self.store_hidden(&key_binding);
                self.code.push(Instruction::GetComputed);
            }
        }
        // `ToNumeric`, not `+ 0` and not unary plus: `+` on a string concatenates, so `o.s++`
        // where `s` is "5" would produce "50", and unary plus narrows a BigInt to a number, which
        // an update must not.
        self.code.push(Instruction::Unary(UnaryOp::ToNumeric));
        self.declare_hidden(&old_binding, span);

        // Write the new value through the same reference the read used.
        self.load_hidden(&receiver);
        if name.is_none() {
            self.load_hidden(&key_binding);
        }
        self.load_hidden(&old_binding);
        self.code.push(Instruction::Unary(if increment {
            UnaryOp::Increment
        } else {
            UnaryOp::Decrement
        }));
        match name {
            Some(name) => self.code.push(self.set_property_instruction(name)),
            None => self.code.push(self.set_computed_instruction()),
        }

        // The store leaves the new value, which is what a prefix update evaluates to; a postfix
        // one discards it and hands back what was there before.
        if !prefix {
            self.code.push(Instruction::Pop);
            self.load_hidden(&old_binding);
        }
        self.scopes.pop().expect("update scope was just pushed");
    }

    /// `object.#name = value`, and the compound forms.
    ///
    /// The same shape as a member assignment with a different pair of instructions, which is the
    /// whole difference: the private ones check that the name is already there.
    fn private_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: &str,
        value: &Expression,
    ) {
        let brand = self.private_brand(name);
        let name = self.constant_index(Constant::String(self.private_key(name)));
        if operator.is_short_circuiting() {
            self.short_circuiting_private(operator, object, name, &brand, value);
            return;
        }
        self.expression(object);
        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup);
            self.push_private_brand(&brand);
            self.code.push(Instruction::GetPrivate(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }
        self.push_private_brand(&brand);
        self.code.push(Instruction::SetPrivate(name));
    }

    /// `object.#name ??= value`, and the `&&=`/`||=` forms.
    ///
    /// The same shape as [`Compiler::short_circuiting_member`] and for the same reason: the
    /// receiver has to survive a decision that may skip both the right-hand side and the store,
    /// so `o().#x ||= v` calls `o` once. Without this the operator fell through to the compound
    /// path, which asks `compound_operator` for the arithmetic a short-circuiting assignment
    /// does not have -- and that is unreachable, so the compiler panicked on valid code.
    fn short_circuiting_private(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        name: u32,
        brand: &Option<String>,
        value: &Expression,
    ) {
        let span = object.span;
        let receiver = format!("%scp{}", self.hidden_bindings);
        let current = format!("%scpcur{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        self.expression(object);
        self.declare_hidden(&receiver, span);

        self.load_hidden(&receiver);
        self.push_private_brand(brand);
        self.code.push(Instruction::GetPrivate(name));
        self.declare_hidden(&current, span);

        self.load_hidden(&current);
        let skip = self.emit_jump(short_circuit_jump(operator));

        self.load_hidden(&receiver);
        self.expression(value);
        self.push_private_brand(brand);
        self.code.push(Instruction::SetPrivate(name));
        let done = self.emit_jump(Instruction::Jump(0));

        // Skipped: the expression evaluates to the value that was already there.
        self.patch_jump(skip);
        self.load_hidden(&current);
        self.patch_jump(done);
        self.scopes.pop().expect("assignment scope was just pushed");
    }

    fn member_assignment(
        &mut self,
        operator: AssignmentOperator,
        object: &Expression,
        property: &str,
        value: &Expression,
    ) {
        let name = self.constant_index(Constant::String(property.to_owned()));
        if operator.is_short_circuiting() {
            self.short_circuiting_member(operator, object, Some(name), None, value);
            return;
        }
        self.expression(object);

        if operator != AssignmentOperator::Assign {
            // Read the current value from the same object reference the write will use.
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::GetProperty(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }

        // `SetProperty` leaves the assigned value, which is what an assignment expression
        // evaluates to.
        self.code.push(self.set_property_instruction(name));
    }

    fn super_member_assignment(
        &mut self,
        operator: AssignmentOperator,
        property: &str,
        value: &Expression,
        span: Span,
    ) {
        self.require_this_binding();
        let name = self.constant_index(Constant::String(property.to_owned()));
        if operator.is_short_circuiting() {
            let cur_binding = format!("%scur{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.load_home_prototype();
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::LoadSuperProperty(name));
            self.declare_hidden(&cur_binding, span);

            self.load_hidden(&cur_binding);
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.code.push(Instruction::Pop);
            self.load_home_prototype();
            self.expression(value);
            self.code.push(self.set_super_property_instruction(name));
            let done = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(skip);
            self.code.push(Instruction::Pop);
            self.load_hidden(&cur_binding);
            self.patch_jump(done);
            self.scopes.pop().expect("scope was just pushed");
            return;
        }

        self.load_home_prototype();
        if operator != AssignmentOperator::Assign {
            self.code.push(Instruction::Dup);
            self.code.push(Instruction::LoadSuperProperty(name));
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));
        } else {
            self.expression(value);
        }
        self.code.push(self.set_super_property_instruction(name));
    }

    fn super_computed_assignment(
        &mut self,
        operator: AssignmentOperator,
        property: &Expression,
        value: &Expression,
        span: Span,
    ) {
        self.require_this_binding();
        if operator.is_short_circuiting() {
            let key_binding = format!("%sckey{}", self.hidden_bindings);
            let cur_binding = format!("%sccur{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.expression(property);
            self.declare_hidden(&key_binding, span);

            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.code.push(Instruction::LoadSuperElement);
            self.declare_hidden(&cur_binding, span);

            self.load_hidden(&cur_binding);
            let skip = self.emit_jump(short_circuit_jump(operator));
            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.expression(value);
            self.code.push(self.set_super_element_instruction());
            let done = self.emit_jump(Instruction::Jump(0));

            self.patch_jump(skip);
            self.load_hidden(&cur_binding);
            self.patch_jump(done);
            self.scopes.pop().expect("scope was just pushed");
            return;
        }

        if operator != AssignmentOperator::Assign {
            let key_binding = format!("%sckey{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.scopes.push(HashMap::new());

            self.expression(property);
            self.declare_hidden(&key_binding, span);

            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.code.push(Instruction::LoadSuperElement);
            self.expression(value);
            self.code
                .push(Instruction::Binary(compound_operator(operator)));

            let val_binding = format!("%scval{}", self.hidden_bindings);
            self.hidden_bindings += 1;
            self.declare_hidden(&val_binding, span);

            self.load_hidden(&key_binding);
            self.load_home_prototype();
            self.load_hidden(&val_binding);
            self.code.push(self.set_super_element_instruction());
            self.scopes.pop().expect("scope was just pushed");
        } else {
            self.expression(property);
            self.load_home_prototype();
            self.expression(value);
            self.code.push(self.set_super_element_instruction());
        }
    }

    /// `f?.()` and `o.m?.()`: call only when the callee is there.
    ///
    /// The callee is read into a hidden local so the nullish test has something to look at
    /// without disturbing the receiver beneath it, and the receiver into another so a method call
    /// keeps its `this` across the jump. Skipping means the arguments are never evaluated, which
    /// is the point: `maybe?.(expensive())` does not compute the argument when `maybe` is absent.
    fn optional_call(&mut self, callee: &Expression, arguments: &[ListElement], span: Span) {
        if self.chains.is_empty() {
            self.error(Some(span), "`?.()` outside of an optional chain");
            self.push_constant(Constant::Undefined);
            return;
        }
        let receiver = format!("%oc{}", self.hidden_bindings);
        let method = format!("%ocfn{}", self.hidden_bindings);
        self.hidden_bindings += 1;
        self.scopes.push(HashMap::new());

        // A method call keeps its receiver; anything else has none and gets `undefined`.
        let has_receiver = match &callee.kind {
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.declare_hidden(&receiver, span);
                self.load_hidden(&receiver);
                let name = self.constant_index(Constant::String(property.clone()));
                self.code.push(Instruction::GetProperty(name));
                true
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.declare_hidden(&receiver, span);
                self.load_hidden(&receiver);
                self.expression(property);
                self.code.push(Instruction::GetComputed);
                true
            }
            _ => {
                self.expression(callee);
                false
            }
        };
        self.declare_hidden(&method, span);

        self.load_hidden(&method);
        let present = self.emit_jump(Instruction::JumpIfNotNullish(0));
        // Absent: the whole chain is undefined, and the arguments are never touched.
        self.push_constant(Constant::Undefined);
        let exit = self.emit_jump(Instruction::Jump(0));
        self.chains
            .last_mut()
            .expect("checked just above")
            .push(exit);

        self.patch_jump(present);
        if has_receiver {
            self.load_hidden(&receiver);
        } else {
            self.push_constant(Constant::Undefined);
        }
        self.load_hidden(&method);
        self.array_literal(arguments, span);
        self.code.push(Instruction::Apply);
        self.scopes.pop().expect("call scope was just pushed");
    }

    /// Whether an argument list contains a spread, and so needs its length computed at runtime.
    fn has_spread(arguments: &[ListElement]) -> bool {
        arguments
            .iter()
            .any(|argument| matches!(argument, ListElement::Spread(_)))
    }

    /// Emit the arguments of a call with no spread, in order, returning the count.
    fn emit_arguments(&mut self, arguments: &[ListElement]) -> u16 {
        for argument in arguments {
            let ListElement::Item(value) = argument else {
                unreachable!("callers check for spreads first");
            };
            self.expression(value);
        }
        arguments.len() as u16
    }

    fn call(&mut self, callee: &Expression, arguments: &[ListElement], optional: bool, span: Span) {
        if optional {
            self.optional_call(callee, arguments, span);
            return;
        }
        let is_console_log = !Self::has_spread(arguments)
            && matches!(self.resolve("console"), Resolution::Unresolved)
            && matches!(
                &callee.kind,
                ExpressionKind::StaticMember { object, property, optional: false }
                    if property == "log"
                        && matches!(&object.kind, ExpressionKind::Identifier(name) if name == "console")
            );
        if arguments.len() > u16::MAX as usize {
            self.error(Some(span), "too many call arguments");
            return;
        }

        if is_console_log {
            // Still a fast compile-time intrinsic when there are no spreads. When spreading,
            // the call falls through to the normal method dispatch on the `console` object.
            let argc = self.emit_arguments(arguments);
            self.code.push(Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc,
            });
            return;
        }

        // A method call keeps its receiver rather than reading the function out first. With a
        // spread the receiver has to be kept explicitly instead, because `Apply` takes it as an
        // operand -- so each shape below has a fixed-arity form and a spread form.
        let spreading = Self::has_spread(arguments);

        // `eval(...)`, written as that name, with nothing shadowing it: a *direct* eval, which
        // evaluates in this scope rather than in the global one. Every other way of reaching the
        // same function -- `(0, eval)(s)`, `globalThis.eval(s)`, a saved reference -- is indirect
        // and compiles to an ordinary call of `Builtin::Eval`. The difference is the language's,
        // not an optimisation: only the direct form can see the names around it.
        if let ExpressionKind::Identifier(name) = &callee.kind
            && name == "eval"
            && self.names_nothing("eval")
        {
            let mut bindings = self.eval_site_bindings(span);
            // The home object travels as an ordinary binding. `eval_site_bindings` skips every
            // name beginning with `%` -- none of them can be written by the eval'd source -- so
            // this is added afterwards rather than found there, and only where the language
            // allows `super.x` to be written at all.
            let home = self
                .super_property
                .then(|| self.home_bindings.last().cloned())
                .flatten();
            if let Some(name) = &home {
                match self.resolve_or_capture(name) {
                    Resolution::Local(local) if local.celled => bindings.push(EvalBinding {
                        name: name.clone(),
                        access: EvalAccess::Cell(local.slot),
                        declaration: EvalDeclaration::Const,
                    }),
                    Resolution::Upvalue(index) => bindings.push(EvalBinding {
                        name: name.clone(),
                        access: EvalAccess::Upvalue(index),
                        declaration: EvalDeclaration::Const,
                    }),
                    // Unreachable: a home binding is declared captured, and `cell_everything`
                    // boxes every binding of a function holding a direct eval. Reported rather
                    // than dropped, because a dropped home would make `super.x` in the eval'd
                    // source read some other slot by number.
                    _ => self.error(Some(span), "a home object binding lost its cell"),
                }
            }
            // A field initialiser is its own function boundary for `new.target` only; `super.x`
            // there reads the class's home object, which is why the two are decided separately.
            let new_target = if self.field_initialiser {
                EvalNewTarget::Undefined
            } else if self.in_function {
                EvalNewTarget::Caller
            } else {
                EvalNewTarget::Forbidden
            };
            let private_scopes = self
                .private_scopes
                .iter()
                .zip(&self.private_declarations)
                .map(|(scope, names)| {
                    let mut names: Vec<String> = names.iter().cloned().collect();
                    names.sort();
                    PrivateScope {
                        scope: *scope,
                        names,
                    }
                })
                .collect();
            let argc = if spreading {
                self.array_literal(arguments, span);
                None
            } else {
                Some(self.emit_arguments(arguments))
            };
            self.code.push(Instruction::DirectEval(Box::new(EvalSite {
                bindings,
                strict: self.strict,
                argc,
                global_var_scope: self.global_var_scope && !self.compiling_function(),
                home,
                new_target,
                private_scopes,
                blocked_var_names: self.parameter_scope.clone().unwrap_or_default(),
            })));
            return;
        }
        match &callee.kind {
            // `super.m(...)` resolves the method on the home object's prototype but runs it
            // against the current receiver, which is precisely the `[this, callee, arguments]`
            // shape `Apply` takes -- no dedicated instruction is needed for either half.
            //
            // The spread-free spelling used `SuperCall` instead, which is *construction*: it
            // treats the value it pops as a parent constructor. That was invisible while
            // `super.x` could only reach a compiled method, and stopped being so the moment it
            // could reach `Object.prototype` -- `super.toString()` in a base class reported that
            // `Object.prototype.toString` is not a constructor. `SuperCall` now means `super()`
            // and nothing else.
            ExpressionKind::SuperMember { .. } | ExpressionKind::SuperComputedMember { .. } => {
                self.load_this();
                self.expression(callee);
                self.array_literal(arguments, span);
                self.code.push(Instruction::Apply);
                return;
            }
            // `this.#m()` is a method call: the private name is read off the receiver and the
            // receiver becomes `this`. Reached through `Apply` rather than a `CallPrivate`
            // instruction, because the shape `Apply` already takes -- receiver, callee,
            // arguments -- is exactly this one.
            ExpressionKind::PrivateMember {
                object,
                name,
                optional,
            } => {
                let brand = self.private_brand(name);
                let name = self.constant_index(Constant::String(self.private_key(name)));
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                self.code.push(Instruction::Dup);
                self.push_private_brand(&brand);
                self.code.push(Instruction::GetPrivate(name));
                self.array_literal(arguments, span);
                self.code.push(Instruction::Apply);
                return;
            }
            ExpressionKind::StaticMember {
                object,
                property,
                optional,
            } => {
                let name = self.constant_index(Constant::String(property.clone()));
                self.expression(object);
                // `a?.b()` short-circuits before the method is even read, so the receiver is the
                // thing tested and the call never happens.
                self.short_circuit_if_nullish(*optional, span);
                if spreading {
                    // The receiver stays beneath the method, which is what `Apply` expects.
                    self.code.push(Instruction::Dup);
                    self.code.push(Instruction::GetProperty(name));
                    self.array_literal(arguments, span);
                    self.code.push(Instruction::Apply);
                } else {
                    let argc = self.emit_arguments(arguments);
                    self.code.push(Instruction::CallMethod { name, argc });
                }
                return;
            }
            ExpressionKind::ComputedMember {
                object,
                property,
                optional,
            } => {
                self.expression(object);
                self.short_circuit_if_nullish(*optional, span);
                if spreading {
                    self.code.push(Instruction::Dup);
                    self.expression(property);
                    self.code.push(Instruction::GetComputed);
                    self.array_literal(arguments, span);
                    self.code.push(Instruction::Apply);
                } else {
                    self.expression(property);
                    let argc = self.emit_arguments(arguments);
                    self.code.push(Instruction::CallComputed { argc });
                }
                return;
            }
            _ => {}
        }

        // Everything else is a real call. The callee is evaluated first so it sits below its
        // arguments, which is both the evaluation order the language requires and the layout
        // the VM needs to find it at a fixed depth.
        if spreading {
            // A plain call has no receiver, and `Apply` takes one, so `undefined` stands in.
            self.push_constant(Constant::Undefined);
            self.expression(callee);
            self.array_literal(arguments, span);
            self.code.push(Instruction::Apply);
            return;
        }
        self.expression(callee);
        let argc = self.emit_arguments(arguments);
        self.code.push(Instruction::Call { argc });
    }

    /// The bindings a direct `eval` written here can see, innermost first.
    ///
    /// Every one is handed over as a *cell*, so the eval'd source shares the binding rather than
    /// a copy of its value: `function f() { var x = 1; eval("x = 2"); return x; }` is 2, and a
    /// copy would make it 1. That is what [`Compiler::cell_everything`] guarantees -- a function
    /// containing an eval boxes all of its bindings, and so does every function around it.
    ///
    /// Resolving each name is what *captures* it, so an enclosing function's binding gains its
    /// upvalue here rather than at a use the eval'd source has not been written yet.
    fn eval_site_bindings(&mut self, span: Span) -> Vec<EvalBinding> {
        // Names first: resolving one mutates the upvalue lists, which cannot happen while the
        // scopes it reads are borrowed. Innermost scope first, so a shadowed name never wins;
        // sorted within a scope, because two bindings of one scope cannot shadow each other and
        // a hash map's order would otherwise make compilation non-reproducible.
        // The declaring `Local` is taken here too. An upvalue cannot say how its binding was
        // declared -- a capture carries no kind -- and the eval'd source needs to know: `var x`
        // over a caller's `var` is that binding, and over a caller's `let` it is a SyntaxError.
        let mut order: Vec<(String, EvalDeclaration)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let levels = self.scopes.iter().rev().chain(
            self.enclosing
                .iter()
                .rev()
                .flat_map(|level| level.scopes.iter().rev()),
        );
        for scope in levels {
            let mut names: Vec<&String> = scope.keys().collect();
            names.sort();
            for name in names {
                // A compiler-introduced binding carries a character an identifier cannot, so it
                // is never a name the eval'd source could write.
                if name.starts_with('%') || !seen.insert(name.clone()) {
                    continue;
                }
                let local = scope[name];
                // An import is the exporter's binding, and writing one is refused wherever it is
                // written. `const` is the nearest thing this vocabulary has to that.
                let declaration = match local.kind {
                    _ if local.imported => EvalDeclaration::Const,
                    VariableKind::Var => EvalDeclaration::Var,
                    VariableKind::Let => EvalDeclaration::Let,
                    // A `using` binding cannot be reassigned either, and what an eval'd source
                    // may do with it is exactly what it may do with a `const`.
                    VariableKind::Const | VariableKind::Using | VariableKind::AwaitUsing => {
                        EvalDeclaration::Const
                    }
                };
                order.push((name.clone(), declaration));
            }
        }

        let mut bindings = Vec::with_capacity(order.len());
        for (name, declaration) in order {
            let access = match self.resolve_or_capture(&name) {
                Resolution::Local(local) if local.celled => EvalAccess::Cell(local.slot),
                Resolution::Upvalue(index) => EvalAccess::Upvalue(index),
                // Unreachable: `cell_everything` boxed every binding in every scope that can
                // reach this call. Refusing rather than dropping the name, because a dropped
                // binding does not disappear -- the eval'd source would resolve it to a built-in
                // or to nothing, and answer with something plausible from the wrong scope.
                _ => {
                    self.error(
                        Some(span),
                        format!("a direct `eval` cannot reach the binding `{name}`"),
                    );
                    continue;
                }
            };
            bindings.push(EvalBinding {
                name,
                access,
                declaration,
            });
        }
        bindings
    }

    /// Names visible to the function being compiled, and how each is reached.
    fn resolve(&self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        let Some(module) = self.enclosing.first() else {
            // Compiling the module body: its scopes were searched above, so the global object is
            // the only place left, and only for a name this program declared there.
            return self.global_object_binding(name);
        };
        if let Some(local) = module.scopes[0].get(name) {
            return Resolution::Global(local.slot);
        }
        self.global_object_binding(name)
    }

    /// Whether `name` is one this program declared on the global object.
    ///
    /// Checked last, after every scope: a `let` at the top of the script, a parameter, a block
    /// binding all shadow it, exactly as they shadow any other global. Checked *before* the
    /// built-in table, though, because `var Object = 1` at the top of a script is that script's
    /// binding and not the intrinsic.
    fn global_object_binding(&self, name: &str) -> Resolution {
        if self.global_bindings.contains(name) {
            Resolution::GlobalObject
        } else {
            Resolution::Unresolved
        }
    }

    /// Resolve `name` and, if it lives in an enclosing function, capture it.
    ///
    /// Split from [`Compiler::resolve`] because capturing mutates the upvalue lists of every
    /// function between the binding and here, and a read-only lookup is wanted in places that
    /// must not have that side effect -- the `console` check in [`Compiler::call`] above all,
    /// which asks whether a name exists and would otherwise capture one by asking.
    fn resolve_or_capture(&mut self, name: &str) -> Resolution {
        if let Some(local) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
            return Resolution::Local(*local);
        }
        // The capture is tried before the global, because an enclosing function's binding
        // shadows a module binding of the same name. Asking `resolve` first would jump from
        // here to module scope and skip every function in between -- reading the wrong binding
        // whenever a name is reused, and reusing a name like `value` is not unusual. That was
        // invisible for as long as a module binding was only created at the line that declared
        // it, because the name did not exist yet when the inner function was compiled.
        if let Some(index) = self.capture(name) {
            return Resolution::Upvalue(index);
        }
        self.resolve(name)
    }

    /// Whether `name` binds nothing anywhere in scope, without creating a capture to find out.
    ///
    /// `resolve` alone is not this question: it looks in the current function and then jumps to
    /// module scope, skipping every function in between. Asking it directly made
    /// `typeof enclosingParameter` answer "undefined" -- which is the wrong answer for a name
    /// that plainly exists, and is exactly what every CommonJS shim tests when it writes
    /// `typeof exports == "object"` inside a nested function.
    fn names_nothing(&self, name: &str) -> bool {
        if self.scopes.iter().any(|scope| scope.contains_key(name)) {
            return false;
        }
        if self
            .enclosing
            .iter()
            .any(|level| level.scopes.iter().any(|scope| scope.contains_key(name)))
        {
            return false;
        }
        matches!(self.resolve(name), Resolution::Unresolved)
    }

    /// Emit the read of a name this program put on the global object.
    ///
    /// `ThrowReference` is the whole of it: it looks the name up on the global object and only
    /// throws when it is not there, which is what a read of a global `var` binding is -- the
    /// property exists from declaration instantiation onwards, so the throw is unreachable for
    /// as long as nothing deletes it, and correct the moment something does.
    fn load_global_object_binding(&mut self, name: &str) {
        let name = self.constant_index(Constant::String(name.to_owned()));
        self.code.push(Instruction::ThrowReference(name));
    }

    fn store_global_object_binding(&mut self, name: &str) {
        let name = self.constant_index(Constant::String(name.to_owned()));
        self.code.push(Instruction::StoreGlobalBinding(name));
    }

    /// Thread a binding from the function that owns it down to the one being compiled.
    ///
    /// Every function in between gains an upvalue too, because a closure can only reach cells
    /// its immediately enclosing frame holds -- a two-level capture is two hops, not one long
    /// one. Levels are indices into `enclosing` followed by the current function.
    ///
    /// Module scope is included. It was skipped while a module binding was reached through the
    /// module frame instead, which cannot survive a function value leaving its module.
    fn capture(&mut self, name: &str) -> Option<u32> {
        // Scope 0 of level 0 is module scope, whose bindings are globals rather than captures --
        // but a *block* inside the module body is an ordinary scope, and a closure created there
        // captures from it like any other. Skipping level 0 entirely missed exactly that: a
        // `for-of` at module level whose body creates a closure over the loop variable.
        let (owner, local) = (0..self.enclosing.len()).rev().find_map(|level| {
            self.enclosing[level]
                .scopes
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
                .map(|local| (level, *local))
        })?;
        if !local.celled {
            // The capture analysis said no nested function referred to this name, yet here we
            // are. Compiling it anyway would read an uninitialised slot as though it were a
            // cell, so refuse rather than emit that.
            return None;
        }

        let name_index = self.constant_index(Constant::String(name.to_owned()));
        let mut source = Upvalue::NamedParentLocal {
            slot: local.slot,
            name: name_index,
        };
        for level in (owner + 1)..=self.enclosing.len() {
            let index = self.add_upvalue(level, name, source);
            source = Upvalue::NamedParentUpvalue {
                index,
                name: name_index,
            };
        }
        // The last index added belongs to the function being compiled.
        self.upvalue_names.get(name).copied()
    }

    /// Add one upvalue to the function at `level`, reusing an existing capture of the same name.
    fn add_upvalue(&mut self, level: usize, name: &str, source: Upvalue) -> u32 {
        let (upvalues, names) = if level == self.enclosing.len() {
            (&mut self.upvalues, &mut self.upvalue_names)
        } else {
            let state = &mut self.enclosing[level];
            (&mut state.upvalues, &mut state.upvalue_names)
        };
        if let Some(existing) = names.get(name) {
            return *existing;
        }
        let index = upvalues.len() as u32;
        upvalues.push(source);
        names.insert(name.to_owned(), index);
        index
    }

    fn compiling_function(&self) -> bool {
        !self.enclosing.is_empty()
    }

    fn push_literal(&mut self, literal: &Literal) {
        self.push_constant(match literal {
            Literal::Undefined => Constant::Undefined,
            Literal::Null => Constant::Null,
            Literal::Boolean(value) => Constant::Boolean(*value),
            Literal::Number(value) => Constant::Number(*value),
            Literal::String(value) => Constant::String(value.clone()),
            Literal::BigInt(digits) => Constant::BigInt(digits.clone()),
        });
    }

    fn push_constant(&mut self, constant: Constant) {
        let index = self.constant_index(constant);
        self.code.push(Instruction::Const(index));
    }

    /// Intern a constant, returning its table index.
    ///
    /// Property names repeat heavily -- every `.length` in a program is the same string -- and
    /// an instruction that names one by index rather than carrying it inline is what lets the
    /// table hold one copy.
    fn constant_index(&mut self, constant: Constant) -> u32 {
        if let Some(existing) = self
            .constants
            .iter()
            .position(|candidate| *candidate == constant)
        {
            return existing as u32;
        }
        let index = self.constants.len() as u32;
        self.constants.push(constant);
        index
    }

    fn emit_jump(&mut self, instruction: Instruction) -> usize {
        let index = self.code.len();
        self.code.push(instruction);
        index
    }

    fn patch_jump(&mut self, index: usize) {
        let target = self.code.len() as u32;
        match &mut self.code[index] {
            Instruction::Jump(offset)
            | Instruction::JumpIfFalse(offset)
            | Instruction::JumpIfTruthy(offset)
            | Instruction::JumpIfNotNullish(offset)
            // A handler's target is patched exactly like a jump's: neither is known until the
            // code it points past has been emitted.
            | Instruction::PushHandler(offset)
            | Instruction::YieldStar(offset)
            | Instruction::JumpIfReturnSignal(offset) => *offset = target,
            other => unreachable!("patch_jump called for {other:?}, which carries no target"),
        }
    }
}

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
fn resource_declarations(list: &[Statement]) -> Vec<bool> {
    let mut resources = Vec::new();
    for statement in list {
        if let StatementKind::Variable { kind, declarations } = &statement.kind
            && let Some(awaiting) = kind.disposal()
        {
            resources.extend(std::iter::repeat_n(awaiting, declarations.len()));
        }
    }
    resources
}

fn removable_class_structure(class: &Class) -> bool {
    class
        .methods
        .iter()
        .all(|method| matches!(method.key, ObjectKey::Static(_) | ObjectKey::Private(_)))
        && class
            .initialisers
            .iter()
            .all(|initialiser| match initialiser {
                ClassInitialiser::Field(field) => {
                    !field.is_static
                        && matches!(field.key, ObjectKey::Static(_) | ObjectKey::Private(_))
                }
                ClassInitialiser::StaticBlock(_) => false,
            })
}

fn removable_class_evaluation(class: &Class) -> bool {
    // Evaluating a superclass reads a binding and validates its current value. Even a name
    // declared as a class elsewhere may be in its TDZ or reassigned before this statement.
    // Keep all heritage evaluation until we can prove initialization and value stability.
    class.superclass.is_none() && removable_class_structure(class)
}

fn removable_declaration_names(statement: &Statement) -> Option<Vec<String>> {
    match &statement.kind {
        StatementKind::Class(class) if removable_class_evaluation(class) => {
            class.name.clone().map(|name| vec![name])
        }
        StatementKind::Variable { kind, declarations }
            if kind.disposal().is_none() && !declarations.is_empty() =>
        {
            let mut names = Vec::with_capacity(declarations.len());
            for declaration in declarations {
                let name = declaration.pattern.as_identifier()?.to_owned();
                let removable = match declaration.initializer.as_ref().map(|value| &value.kind) {
                    None | Some(ExpressionKind::Function(_)) => true,
                    Some(ExpressionKind::Class(class)) => removable_class_evaluation(class),
                    _ => false,
                };
                if !removable {
                    return None;
                }
                names.push(name);
            }
            Some(names)
        }
        _ => None,
    }
}

pub fn mutable_module_bindings(body: &[Statement]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in body {
        if let StatementKind::Variable { kind, declarations } = &statement.kind
            && *kind != VariableKind::Const
        {
            for declaration in declarations {
                collect_pattern_names(&declaration.pattern, &mut names);
            }
        }
        collect_assigned_names(statement, &mut names);
    }
    names
}

/// Identify exported and top-level module bindings that are never mutated.
pub fn immutable_module_bindings(module: &Module) -> Vec<String> {
    let mutable = mutable_module_bindings(&module.body);
    let mut names = HashSet::new();
    for statement in &module.body {
        match &statement.kind {
            StatementKind::Variable { declarations, .. } => {
                for declaration in declarations {
                    collect_pattern_names(&declaration.pattern, &mut names);
                }
            }
            StatementKind::Function(function) => {
                if let Some(name) = &function.name {
                    names.insert(name.clone());
                }
            }
            StatementKind::Class(class) => {
                if let Some(name) = &class.name {
                    names.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    for import in &module.imports {
        names.insert(import.local.clone());
    }
    for export in &module.exports {
        if let ExportOrigin::Local { local } = &export.origin {
            names.insert(local.clone());
        }
    }
    let mut immutable: Vec<String> = names
        .into_iter()
        .filter(|name| !mutable.contains(name))
        .collect();
    immutable.sort();
    immutable
}

/// Every name assigned or updated anywhere in `statement`, however deeply nested.
///
/// Deliberately an over-approximation, like the capture analysis: a name boxed that never changes
/// costs one pointer chase, where a name *missed* would read as a stale value through an import.
fn collect_assigned_names(statement: &Statement, names: &mut HashSet<String>) {
    let mut expressions: Vec<&Expression> = Vec::new();
    let mut statements: Vec<&Statement> = vec![statement];
    while let Some(statement) = statements.pop() {
        match &statement.kind {
            StatementKind::Variable { declarations, .. } => {
                for declaration in declarations {
                    if let Some(initializer) = &declaration.initializer {
                        expressions.push(initializer);
                    }
                }
            }
            StatementKind::Function(function) => statements.extend(function.body.iter()),
            StatementKind::Class(class) => {
                for method in &class.methods {
                    statements.extend(method.function.body.iter());
                }
                if let Some(constructor) = &class.constructor {
                    statements.extend(constructor.body.iter());
                }
            }
            StatementKind::Expression(expression)
            | StatementKind::Throw(expression)
            | StatementKind::ExportDefault(expression) => expressions.push(expression),
            StatementKind::Return(value) => expressions.extend(value.iter()),
            StatementKind::Block(body) => statements.extend(body.iter()),
            StatementKind::If {
                test,
                consequent,
                alternate,
            } => {
                expressions.push(test);
                statements.push(consequent);
                statements.extend(alternate.iter().map(std::convert::AsRef::as_ref));
            }
            StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
                expressions.push(test);
                statements.push(body);
            }
            StatementKind::For {
                init,
                test,
                update,
                body,
            } => {
                statements.extend(init.iter().map(std::convert::AsRef::as_ref));
                expressions.extend(test.iter());
                expressions.extend(update.iter());
                statements.push(body);
            }
            StatementKind::ForOf { iterable, body, .. } => {
                expressions.push(iterable);
                statements.push(body);
            }
            StatementKind::ForIn { object, body, .. } => {
                expressions.push(object);
                statements.push(body);
            }
            StatementKind::Labelled { body, .. } => statements.push(body),
            StatementKind::With { object, body } => {
                expressions.push(object);
                statements.push(body);
            }
            StatementKind::Switch {
                discriminant,
                cases,
            } => {
                expressions.push(discriminant);
                for case in cases {
                    expressions.extend(case.test.iter());
                    statements.extend(case.body.iter());
                }
            }
            StatementKind::Try {
                block,
                handler,
                finalizer,
            } => {
                statements.extend(block.iter());
                if let Some(handler) = handler {
                    statements.extend(handler.body.iter());
                }
                statements.extend(finalizer.iter());
            }
            StatementKind::Break { .. }
            | StatementKind::Continue { .. }
            | StatementKind::Empty
            | StatementKind::Unsupported { .. } => {}
        }
    }
    for expression in expressions {
        collect_identifiers(expression, names, true);
    }
}

/// Names that nested functions inside `body` refer to.
///
/// Deliberately an over-approximation: it collects every identifier appearing anywhere inside a
/// nested function, without modelling that function's own shadowing. A local that is boxed but
/// never actually captured still behaves correctly -- one pointer chase, no semantic difference
/// -- whereas a captured local that was *missed* would be read out of a dead frame. The analysis
/// is allowed to be wrong only in the direction that costs performance.
fn names_captured_by_nested_functions(body: &[Statement]) -> HashSet<String> {
    let mut captured = HashSet::new();
    for statement in body {
        collect_nested_function_names(statement, &mut captured, false);
    }
    captured
}

/// As [`names_captured_by_nested_functions`], but for a whole function rather than a body.
///
/// A parameter default is compiled in the function's own scope and may contain a nested function
/// of its own -- `function f(a, cb = () => a)` captures `a`, and nothing in the body says so. The
/// body-only scan leaves `a` unboxed, and the arrow then cannot reach it at all.
fn names_captured_by_this_function(function: &Function) -> HashSet<String> {
    let mut captured = names_captured_by_nested_functions(&function.body);
    for parameter in &function.parameters {
        // A parameter default runs in this function's own scope. Ordinary identifiers in it may
        // therefore refer outward and must be captured by this function itself, not only when a
        // second nested function appears inside the pattern. `path-to-regexp` relies on exactly
        // this shape: `function match(path, { delimiter = DEFAULT_DELIMITER } = {})`.
        collect_pattern_identifiers(&parameter.pattern, &mut captured);
    }
    if let Some(rest) = &function.rest {
        collect_pattern_identifiers(rest, &mut captured);
    }
    captured
}

/// Whether `body` contains a syntactically direct `eval(...)` call.
///
/// This deliberately looks through nested functions. An eval in a nested function can still name
/// a binding owned by any enclosing function, so every enclosing level has to box its bindings as
/// well. Shadowing is left to normal resolution when the call itself is compiled; treating a
/// shadowed `eval` as direct here merely boxes a few extra locals and cannot change their values.
pub fn body_contains_direct_eval(body: &[Statement]) -> bool {
    body.iter().any(statement_contains_direct_eval)
}

pub fn function_contains_direct_eval(function: &Function) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| pattern_contains_direct_eval(&parameter.pattern))
        || function
            .rest
            .as_ref()
            .is_some_and(pattern_contains_direct_eval)
        || body_contains_direct_eval(&function.body)
}

pub fn module_contains_direct_eval(module: &Module) -> bool {
    body_contains_direct_eval(&module.body)
}

fn statement_contains_direct_eval(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => declarations.iter().any(|declaration| {
            pattern_contains_direct_eval(&declaration.pattern)
                || declaration
                    .initializer
                    .as_ref()
                    .is_some_and(expression_contains_direct_eval)
        }),
        StatementKind::Function(function) => function_contains_direct_eval(function),
        StatementKind::Class(class) => class_contains_direct_eval(class),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => expression_contains_direct_eval(expression),
        StatementKind::Return(value) => value.as_ref().is_some_and(expression_contains_direct_eval),
        StatementKind::Block(body) => body_contains_direct_eval(body),
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_direct_eval(test)
                || statement_contains_direct_eval(consequent)
                || alternate
                    .as_deref()
                    .is_some_and(statement_contains_direct_eval)
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            expression_contains_direct_eval(test) || statement_contains_direct_eval(body)
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_deref().is_some_and(statement_contains_direct_eval)
                || test.as_ref().is_some_and(expression_contains_direct_eval)
                || update.as_ref().is_some_and(expression_contains_direct_eval)
                || statement_contains_direct_eval(body)
        }
        StatementKind::ForOf { iterable, body, .. } => {
            expression_contains_direct_eval(iterable) || statement_contains_direct_eval(body)
        }
        StatementKind::ForIn { object, body, .. } => {
            expression_contains_direct_eval(object) || statement_contains_direct_eval(body)
        }
        StatementKind::Labelled { body, .. } => statement_contains_direct_eval(body),
        StatementKind::With { object, body } => {
            expression_contains_direct_eval(object) || statement_contains_direct_eval(body)
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            expression_contains_direct_eval(discriminant)
                || cases.iter().any(|case| {
                    case.test
                        .as_ref()
                        .is_some_and(expression_contains_direct_eval)
                        || body_contains_direct_eval(&case.body)
                })
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            body_contains_direct_eval(block)
                || handler
                    .as_ref()
                    .is_some_and(|handler| body_contains_direct_eval(&handler.body))
                || body_contains_direct_eval(finalizer)
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => false,
    }
}

fn class_contains_direct_eval(class: &Class) -> bool {
    class
        .superclass
        .as_ref()
        .is_some_and(expression_contains_direct_eval)
        || class
            .constructor
            .as_ref()
            .is_some_and(function_contains_direct_eval)
        || class
            .methods
            .iter()
            .any(|method| function_contains_direct_eval(&method.function))
        || class.initialisers.iter().any(|initialiser| match initialiser {
            ClassInitialiser::Field(field) => {
                matches!(&field.key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || field
                        .value
                        .as_ref()
                        .is_some_and(expression_contains_direct_eval)
            }
            // A static block is a body like any other, and a direct `eval` inside it sees the
            // same scope the block does.
            ClassInitialiser::StaticBlock(body) => function_contains_direct_eval(body),
        })
}

fn pattern_contains_direct_eval(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Identifier(_) => false,
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            expression_contains_direct_eval(object)
        }
        Pattern::Computed { object, property } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(property)
        }
        Pattern::WithDefault { inner, default } => {
            pattern_contains_direct_eval(inner) || expression_contains_direct_eval(default)
        }
        Pattern::Object { properties, rest } => {
            properties.iter().any(|property| {
                matches!(&property.key, PatternKey::Computed(key) if expression_contains_direct_eval(key))
                    || pattern_contains_direct_eval(&property.value)
            }) || rest
                .as_deref()
                .is_some_and(pattern_contains_direct_eval)
        }
        Pattern::Array { elements, rest } => {
            elements
                .iter()
                .flatten()
                .any(pattern_contains_direct_eval)
                || rest
                    .as_deref()
                    .is_some_and(pattern_contains_direct_eval)
        }
    }
}

fn expression_contains_direct_eval(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call {
            callee,
            arguments,
            optional,
        } => {
            (!optional
                && matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "eval"))
                || expression_contains_direct_eval(callee)
                || arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Function(function) => function_contains_direct_eval(function),
        ExpressionKind::Class(class) => class_contains_direct_eval(class),
        ExpressionKind::ImportCall { specifier, options } => {
            expression_contains_direct_eval(specifier)
                || options
                    .as_deref()
                    .is_some_and(expression_contains_direct_eval)
        }
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => expression_contains_direct_eval(argument),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_contains_direct_eval(left) || expression_contains_direct_eval(right)
        }
        ExpressionKind::Assignment { value, .. } => expression_contains_direct_eval(value),
        ExpressionKind::PatternAssignment { pattern, value } => {
            pattern_contains_direct_eval(pattern) || expression_contains_direct_eval(value)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_direct_eval(test)
                || expression_contains_direct_eval(consequent)
                || expression_contains_direct_eval(alternate)
        }
        ExpressionKind::New { callee, arguments } => {
            expression_contains_direct_eval(callee)
                || arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::SuperCall { arguments } => {
            arguments.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Array { elements } => {
            elements.iter().any(list_element_contains_direct_eval)
        }
        ExpressionKind::Sequence { operands } => {
            operands.iter().any(expression_contains_direct_eval)
        }
        ExpressionKind::Yield { value, .. } => value
            .as_ref()
            .is_some_and(|value| expression_contains_direct_eval(value)),
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(property)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_contains_direct_eval(object)
                || expression_contains_direct_eval(property)
                || expression_contains_direct_eval(value)
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            expression_contains_direct_eval(object) || expression_contains_direct_eval(value)
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            expression_contains_direct_eval(value)
        }
        ExpressionKind::SuperComputedAssignment { property, value, .. } => {
            expression_contains_direct_eval(property) || expression_contains_direct_eval(value)
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            expression_contains_direct_eval(object)
                || matches!(property.as_ref(), ObjectKey::Computed(key) if expression_contains_direct_eval(key))
        }
        ExpressionKind::DeleteValue { argument } => expression_contains_direct_eval(argument),
        ExpressionKind::DeleteBinding { .. } => false,
        ExpressionKind::Template { expressions, .. } => {
            expressions.iter().any(expression_contains_direct_eval)
        }
        ExpressionKind::Object { members } => members.iter().any(|member| match member {
            ObjectMember::Property(property) => {
                matches!(&property.key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || expression_contains_direct_eval(&property.value)
            }
            ObjectMember::Spread(source) => expression_contains_direct_eval(source),
            ObjectMember::Accessor { key, function, .. } => {
                matches!(key, ObjectKey::Computed(key) if expression_contains_direct_eval(key))
                    || function_contains_direct_eval(function)
            }
        }),
        ExpressionKind::SuperComputedMember { property } => {
            expression_contains_direct_eval(property)
        }
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => false,
    }
}

fn list_element_contains_direct_eval(element: &ListElement) -> bool {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            expression_contains_direct_eval(value)
        }
        ListElement::Hole => false,
    }
}

/// Whether `body` contains `new Function(...)` or dynamic `Function(...)`.
pub fn body_contains_new_function(body: &[Statement]) -> bool {
    body.iter().any(statement_contains_new_function)
}

pub fn function_contains_new_function(function: &Function) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| pattern_contains_new_function(&parameter.pattern))
        || function
            .rest
            .as_ref()
            .is_some_and(pattern_contains_new_function)
        || body_contains_new_function(&function.body)
}

pub fn module_contains_new_function(module: &Module) -> bool {
    body_contains_new_function(&module.body)
}

fn statement_contains_new_function(statement: &Statement) -> bool {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => declarations.iter().any(|declaration| {
            pattern_contains_new_function(&declaration.pattern)
                || declaration
                    .initializer
                    .as_ref()
                    .is_some_and(expression_contains_new_function)
        }),
        StatementKind::Function(function) => function_contains_new_function(function),
        StatementKind::Class(class) => class_contains_new_function(class),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => expression_contains_new_function(expression),
        StatementKind::Return(value) => {
            value.as_ref().is_some_and(expression_contains_new_function)
        }
        StatementKind::Block(body) => body_contains_new_function(body),
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_new_function(test)
                || statement_contains_new_function(consequent)
                || alternate
                    .as_deref()
                    .is_some_and(statement_contains_new_function)
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            expression_contains_new_function(test) || statement_contains_new_function(body)
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            init.as_deref().is_some_and(statement_contains_new_function)
                || test.as_ref().is_some_and(expression_contains_new_function)
                || update
                    .as_ref()
                    .is_some_and(expression_contains_new_function)
                || statement_contains_new_function(body)
        }
        StatementKind::ForOf { iterable, body, .. } => {
            expression_contains_new_function(iterable) || statement_contains_new_function(body)
        }
        StatementKind::ForIn { object, body, .. } => {
            expression_contains_new_function(object) || statement_contains_new_function(body)
        }
        StatementKind::Labelled { body, .. } => statement_contains_new_function(body),
        StatementKind::With { object, body } => {
            expression_contains_new_function(object) || statement_contains_new_function(body)
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            expression_contains_new_function(discriminant)
                || cases.iter().any(|case| {
                    case.test
                        .as_ref()
                        .is_some_and(expression_contains_new_function)
                        || body_contains_new_function(&case.body)
                })
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            body_contains_new_function(block)
                || handler
                    .as_ref()
                    .is_some_and(|handler| body_contains_new_function(&handler.body))
                || body_contains_new_function(finalizer)
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => false,
    }
}

fn class_contains_new_function(class: &Class) -> bool {
    class
        .superclass
        .as_ref()
        .is_some_and(expression_contains_new_function)
        || class
            .constructor
            .as_ref()
            .is_some_and(function_contains_new_function)
        || class
            .methods
            .iter()
            .any(|method| function_contains_new_function(&method.function))
        || class.initialisers.iter().any(|initialiser| match initialiser {
            ClassInitialiser::Field(field) => {
                matches!(&field.key, ObjectKey::Computed(key) if expression_contains_new_function(key))
                    || field
                        .value
                        .as_ref()
                        .is_some_and(expression_contains_new_function)
            }
            ClassInitialiser::StaticBlock(body) => function_contains_new_function(body),
        })
}

fn pattern_contains_new_function(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Identifier(_) => false,
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            expression_contains_new_function(object)
        }
        Pattern::Computed { object, property } => {
            expression_contains_new_function(object) || expression_contains_new_function(property)
        }
        Pattern::WithDefault { inner, default } => {
            pattern_contains_new_function(inner) || expression_contains_new_function(default)
        }
        Pattern::Object { properties, rest } => {
            properties.iter().any(|property| {
                matches!(&property.key, PatternKey::Computed(key) if expression_contains_new_function(key))
                    || pattern_contains_new_function(&property.value)
            }) || rest
                .as_deref()
                .is_some_and(pattern_contains_new_function)
        }
        Pattern::Array { elements, rest } => {
            elements
                .iter()
                .flatten()
                .any(pattern_contains_new_function)
                || rest
                    .as_deref()
                    .is_some_and(pattern_contains_new_function)
        }
    }
}

fn expression_contains_new_function(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Call {
            callee,
            arguments,
            ..
        } => {
            matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "Function")
                || expression_contains_new_function(callee)
                || arguments.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::New { callee, arguments } => {
            matches!(&callee.kind, ExpressionKind::Identifier(name) if name == "Function")
                || expression_contains_new_function(callee)
                || arguments.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::Function(function) => function_contains_new_function(function),
        ExpressionKind::Class(class) => class_contains_new_function(class),
        ExpressionKind::ImportCall { specifier, options } => {
            expression_contains_new_function(specifier)
                || options
                    .as_deref()
                    .is_some_and(expression_contains_new_function)
        }
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => expression_contains_new_function(argument),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            expression_contains_new_function(left) || expression_contains_new_function(right)
        }
        ExpressionKind::Assignment { value, .. } => expression_contains_new_function(value),
        ExpressionKind::PatternAssignment { pattern, value } => {
            pattern_contains_new_function(pattern) || expression_contains_new_function(value)
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            expression_contains_new_function(test)
                || expression_contains_new_function(consequent)
                || expression_contains_new_function(alternate)
        }
        ExpressionKind::SuperCall { arguments } => {
            arguments.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::Array { elements } => {
            elements.iter().any(list_element_contains_new_function)
        }
        ExpressionKind::Sequence { operands } => {
            operands.iter().any(expression_contains_new_function)
        }
        ExpressionKind::Yield { value, .. } => value
            .as_ref()
            .is_some_and(|value| expression_contains_new_function(value)),
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            expression_contains_new_function(object) || expression_contains_new_function(property)
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            expression_contains_new_function(object)
                || expression_contains_new_function(property)
                || expression_contains_new_function(value)
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            expression_contains_new_function(object) || expression_contains_new_function(value)
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            expression_contains_new_function(value)
        }
        ExpressionKind::SuperComputedAssignment { property, value, .. } => {
            expression_contains_new_function(property) || expression_contains_new_function(value)
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            expression_contains_new_function(object)
                || matches!(property.as_ref(), ObjectKey::Computed(key) if expression_contains_new_function(key))
        }
        ExpressionKind::DeleteValue { argument } => expression_contains_new_function(argument),
        ExpressionKind::DeleteBinding { .. } => false,
        ExpressionKind::Template { expressions, .. } => {
            expressions.iter().any(expression_contains_new_function)
        }
        ExpressionKind::Object { members } => members.iter().any(|member| match member {
            ObjectMember::Property(property) => {
                matches!(&property.key, ObjectKey::Computed(key) if expression_contains_new_function(key))
                    || expression_contains_new_function(&property.value)
            }
            ObjectMember::Spread(source) => expression_contains_new_function(source),
            ObjectMember::Accessor { key, function, .. } => {
                matches!(key, ObjectKey::Computed(key) if expression_contains_new_function(key))
                    || function_contains_new_function(function)
            }
        }),
        ExpressionKind::SuperComputedMember { property } => {
            expression_contains_new_function(property)
        }
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => false,
    }
}

fn list_element_contains_new_function(element: &ListElement) -> bool {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            expression_contains_new_function(value)
        }
        ListElement::Hole => false,
    }
}

/// Collect all dynamic imports from `body`.
pub fn collect_dynamic_imports(body: &[Statement]) -> Vec<DynamicImportTarget> {
    let mut imports = Vec::new();
    for statement in body {
        collect_dynamic_imports_from_statement(statement, &mut imports);
    }
    imports
}

fn collect_dynamic_imports_from_statement(
    statement: &Statement,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match &statement.kind {
        StatementKind::Variable { declarations, .. } => {
            for declaration in declarations {
                collect_dynamic_imports_from_pattern(&declaration.pattern, imports);
                if let Some(init) = &declaration.initializer {
                    collect_dynamic_imports_from_expression(init, imports);
                }
            }
        }
        StatementKind::Function(function) => {
            collect_dynamic_imports_from_function(function, imports)
        }
        StatementKind::Class(class) => collect_dynamic_imports_from_class(class, imports),
        StatementKind::Expression(expression)
        | StatementKind::Throw(expression)
        | StatementKind::ExportDefault(expression) => {
            collect_dynamic_imports_from_expression(expression, imports);
        }
        StatementKind::Return(value) => {
            if let Some(v) = value {
                collect_dynamic_imports_from_expression(v, imports);
            }
        }
        StatementKind::Block(body) => {
            for s in body {
                collect_dynamic_imports_from_statement(s, imports);
            }
        }
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            collect_dynamic_imports_from_expression(test, imports);
            collect_dynamic_imports_from_statement(consequent, imports);
            if let Some(alt) = alternate {
                collect_dynamic_imports_from_statement(alt, imports);
            }
        }
        StatementKind::While { test, body } | StatementKind::DoWhile { test, body } => {
            collect_dynamic_imports_from_expression(test, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(i) = init {
                collect_dynamic_imports_from_statement(i, imports);
            }
            if let Some(t) = test {
                collect_dynamic_imports_from_expression(t, imports);
            }
            if let Some(u) = update {
                collect_dynamic_imports_from_expression(u, imports);
            }
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::ForOf { iterable, body, .. } => {
            collect_dynamic_imports_from_expression(iterable, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::ForIn { object, body, .. } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::Labelled { body, .. } => {
            collect_dynamic_imports_from_statement(body, imports)
        }
        StatementKind::With { object, body } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_statement(body, imports);
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            collect_dynamic_imports_from_expression(discriminant, imports);
            for case in cases {
                if let Some(test) = &case.test {
                    collect_dynamic_imports_from_expression(test, imports);
                }
                for s in &case.body {
                    collect_dynamic_imports_from_statement(s, imports);
                }
            }
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            for s in block {
                collect_dynamic_imports_from_statement(s, imports);
            }
            if let Some(h) = handler {
                for s in &h.body {
                    collect_dynamic_imports_from_statement(s, imports);
                }
            }
            for s in finalizer {
                collect_dynamic_imports_from_statement(s, imports);
            }
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => {}
    }
}

fn collect_dynamic_imports_from_function(
    function: &Function,
    imports: &mut Vec<DynamicImportTarget>,
) {
    for param in &function.parameters {
        collect_dynamic_imports_from_pattern(&param.pattern, imports);
    }
    if let Some(rest) = &function.rest {
        collect_dynamic_imports_from_pattern(rest, imports);
    }
    for s in &function.body {
        collect_dynamic_imports_from_statement(s, imports);
    }
}

fn collect_dynamic_imports_from_class(class: &Class, imports: &mut Vec<DynamicImportTarget>) {
    if let Some(superclass) = &class.superclass {
        collect_dynamic_imports_from_expression(superclass, imports);
    }
    if let Some(ctor) = &class.constructor {
        collect_dynamic_imports_from_function(ctor, imports);
    }
    for method in &class.methods {
        collect_dynamic_imports_from_function(&method.function, imports);
    }
    for initialiser in &class.initialisers {
        match initialiser {
            ClassInitialiser::Field(field) => {
                if let ObjectKey::Computed(key) = &field.key {
                    collect_dynamic_imports_from_expression(key, imports);
                }
                if let Some(value) = &field.value {
                    collect_dynamic_imports_from_expression(value, imports);
                }
            }
            ClassInitialiser::StaticBlock(body) => {
                collect_dynamic_imports_from_function(body, imports);
            }
        }
    }
}

fn collect_dynamic_imports_from_pattern(pattern: &Pattern, imports: &mut Vec<DynamicImportTarget>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_dynamic_imports_from_expression(object, imports);
        }
        Pattern::Computed { object, property } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(property, imports);
        }
        Pattern::WithDefault { inner, default } => {
            collect_dynamic_imports_from_pattern(inner, imports);
            collect_dynamic_imports_from_expression(default, imports);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_dynamic_imports_from_expression(key, imports);
                }
                collect_dynamic_imports_from_pattern(&property.value, imports);
            }
            if let Some(r) = rest {
                collect_dynamic_imports_from_pattern(r, imports);
            }
        }
        Pattern::Array { elements, rest } => {
            for el in elements.iter().flatten() {
                collect_dynamic_imports_from_pattern(el, imports);
            }
            if let Some(r) = rest {
                collect_dynamic_imports_from_pattern(r, imports);
            }
        }
    }
}

fn collect_dynamic_imports_from_expression(
    expression: &Expression,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match &expression.kind {
        ExpressionKind::ImportCall { specifier, options } => {
            let target = match &specifier.kind {
                ExpressionKind::Literal(Literal::String(s)) => {
                    DynamicImportTarget::Static(s.clone())
                }
                ExpressionKind::Template {
                    quasis,
                    expressions,
                } if expressions.is_empty() && quasis.len() == 1 => {
                    DynamicImportTarget::Static(quasis[0].clone())
                }
                _ => DynamicImportTarget::Dynamic,
            };
            imports.push(target);
            collect_dynamic_imports_from_expression(specifier, imports);
            if let Some(opts) = options {
                collect_dynamic_imports_from_expression(opts, imports);
            }
        }
        ExpressionKind::Call {
            callee, arguments, ..
        } => {
            collect_dynamic_imports_from_expression(callee, imports);
            for arg in arguments {
                collect_dynamic_imports_from_list_element(arg, imports);
            }
        }
        ExpressionKind::New { callee, arguments } => {
            collect_dynamic_imports_from_expression(callee, imports);
            for arg in arguments {
                collect_dynamic_imports_from_list_element(arg, imports);
            }
        }
        ExpressionKind::Function(function) => {
            collect_dynamic_imports_from_function(function, imports)
        }
        ExpressionKind::Class(class) => collect_dynamic_imports_from_class(class, imports),
        ExpressionKind::Unary { argument, .. }
        | ExpressionKind::Chain {
            expression: argument,
        }
        | ExpressionKind::Await(argument)
        | ExpressionKind::PrivateIn {
            object: argument, ..
        }
        | ExpressionKind::PrivateMember {
            object: argument, ..
        }
        | ExpressionKind::StaticMember {
            object: argument, ..
        } => {
            collect_dynamic_imports_from_expression(argument, imports);
        }
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            collect_dynamic_imports_from_expression(left, imports);
            collect_dynamic_imports_from_expression(right, imports);
        }
        ExpressionKind::Assignment { value, .. } => {
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::PatternAssignment { pattern, value } => {
            collect_dynamic_imports_from_pattern(pattern, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            collect_dynamic_imports_from_expression(test, imports);
            collect_dynamic_imports_from_expression(consequent, imports);
            collect_dynamic_imports_from_expression(alternate, imports);
        }
        ExpressionKind::SuperCall { arguments } => {
            for arg in arguments {
                collect_dynamic_imports_from_list_element(arg, imports);
            }
        }
        ExpressionKind::Array { elements } => {
            for el in elements {
                collect_dynamic_imports_from_list_element(el, imports);
            }
        }
        ExpressionKind::Sequence { operands } => {
            for op in operands {
                collect_dynamic_imports_from_expression(op, imports);
            }
        }
        ExpressionKind::Yield { value, .. } => {
            if let Some(v) = value {
                collect_dynamic_imports_from_expression(v, imports);
            }
        }
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(property, imports);
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(property, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::MemberAssignment { object, value, .. }
        | ExpressionKind::PrivateAssignment { object, value, .. } => {
            collect_dynamic_imports_from_expression(object, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::SuperComputedAssignment {
            property, value, ..
        } => {
            collect_dynamic_imports_from_expression(property, imports);
            collect_dynamic_imports_from_expression(value, imports);
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        }
        | ExpressionKind::Delete { object, property } => {
            collect_dynamic_imports_from_expression(object, imports);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_dynamic_imports_from_expression(key, imports);
            }
        }
        ExpressionKind::DeleteValue { argument } => {
            collect_dynamic_imports_from_expression(argument, imports);
        }
        ExpressionKind::DeleteBinding { .. } => {}
        ExpressionKind::Template { expressions, .. } => {
            for expr in expressions {
                collect_dynamic_imports_from_expression(expr, imports);
            }
        }
        ExpressionKind::Object { members } => {
            for member in members {
                match member {
                    ObjectMember::Property(prop) => {
                        if let ObjectKey::Computed(key) = &prop.key {
                            collect_dynamic_imports_from_expression(key, imports);
                        }
                        collect_dynamic_imports_from_expression(&prop.value, imports);
                    }
                    ObjectMember::Spread(source) => {
                        collect_dynamic_imports_from_expression(source, imports);
                    }
                    ObjectMember::Accessor { key, function, .. } => {
                        if let ObjectKey::Computed(key) = key {
                            collect_dynamic_imports_from_expression(key, imports);
                        }
                        collect_dynamic_imports_from_function(function, imports);
                    }
                }
            }
        }
        ExpressionKind::SuperComputedMember { property } => {
            collect_dynamic_imports_from_expression(property, imports);
        }
        ExpressionKind::Identifier(_)
        | ExpressionKind::Update { .. }
        | ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::RegExp { .. }
        | ExpressionKind::TemplateStrings { .. }
        | ExpressionKind::Unsupported { .. } => {}
    }
}

fn collect_dynamic_imports_from_list_element(
    element: &ListElement,
    imports: &mut Vec<DynamicImportTarget>,
) {
    match element {
        ListElement::Item(value) | ListElement::Spread(value) => {
            collect_dynamic_imports_from_expression(value, imports);
        }
        ListElement::Hole => {}
    }
}

/// Names that nested functions inside a pattern's expressions refer to.
///
/// A pattern reads names as well as binding them: a default, a computed key, and the object a
/// reference target stores through are all ordinary expressions, and a closure in any of them
/// decides whether the name it reaches has to be boxed.
fn collect_pattern_captures(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_identifiers(object, names, false);
        }
        Pattern::Computed { object, property } => {
            collect_identifiers(object, names, false);
            collect_identifiers(property, names, false);
        }
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, false);
            collect_pattern_captures(inner, names);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, false);
                }
                collect_pattern_captures(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_captures(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_captures(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_captures(rest, names);
            }
        }
    }
}

/// Every identifier evaluated while a pattern is initialised.
///
/// Unlike [`collect_pattern_captures`], this starts in collecting mode: the pattern itself is
/// already inside a nested function relative to its owner. A function parameter default such as
/// `function f(x = f.value) {}` therefore captures the outer `f` binding even though the
/// function body never mentions it.
fn collect_pattern_identifiers(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(_) => {}
        Pattern::Member { object, .. } | Pattern::PrivateMember { object, .. } => {
            collect_identifiers(object, names, true);
        }
        Pattern::Computed { object, property } => {
            collect_identifiers(object, names, true);
            collect_identifiers(property, names, true);
        }
        Pattern::WithDefault { inner, default } => {
            collect_identifiers(default, names, true);
            collect_pattern_identifiers(inner, names);
        }
        Pattern::Object { properties, rest } => {
            for property in properties {
                if let PatternKey::Computed(key) = &property.key {
                    collect_identifiers(key, names, true);
                }
                // `{ delimiter = DEFAULT_DELIMITER }` stores the default on the property value
                // pattern, so recurse into that pattern's expressions as well as its computed
                // key. Merely walking the bound name misses the initializer entirely.
                collect_pattern_identifiers(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_identifiers(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_identifiers(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_identifiers(rest, names);
            }
        }
    }
}

/// Whether a target stores through a property reference rather than binding a name.
fn stores_through_reference(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => true,
        Pattern::WithDefault { inner, .. } => stores_through_reference(inner),
        _ => false,
    }
}

fn collect_nested_function_names(
    statement: &Statement,
    names: &mut HashSet<String>,
    inside_function: bool,
) {
    match &statement.kind {
        StatementKind::Function(function) => {
            for parameter in &function.parameters {
                collect_pattern_identifiers(&parameter.pattern, names);
            }
            if let Some(rest) = &function.rest {
                collect_pattern_identifiers(rest, names);
            }
            for statement in &function.body {
                collect_nested_function_names(statement, names, true);
            }
        }
        // Every method body is a nested function, so its free names are capture candidates.
        StatementKind::Class(class) => {
            if let Some(superclass) = &class.superclass {
                collect_identifiers(superclass, names, inside_function);
            }
            collect_class_names(class, names);
        }
        StatementKind::Variable { declarations, .. } => {
            for declaration in declarations {
                if let Some(initializer) = &declaration.initializer {
                    collect_identifiers(initializer, names, inside_function);
                }
                // The *pattern* reads names too, and only the initializer was ever walked. A
                // destructuring default and a computed key are both ordinary expressions
                // evaluated in this scope -- `const { a = n } = {}` inside a nested function
                // reads `n` from the enclosing one -- so a binding named only there was left
                // unboxed and the nested function could not resolve it at all.
                //
                // `collect_pattern_identifiers` records what a pattern *reads*, never what it
                // binds, which is why walking it here cannot box the declaration's own names.
                if inside_function {
                    collect_pattern_identifiers(&declaration.pattern, names);
                }
            }
        }
        StatementKind::Expression(expression) | StatementKind::ExportDefault(expression) => {
            collect_identifiers(expression, names, inside_function);
        }
        StatementKind::Return(value) => {
            if let Some(value) = value {
                collect_identifiers(value, names, inside_function);
            }
        }
        StatementKind::Block(body) => {
            for statement in body {
                collect_nested_function_names(statement, names, inside_function);
            }
        }
        StatementKind::If {
            test,
            consequent,
            alternate,
        } => {
            collect_identifiers(test, names, inside_function);
            collect_nested_function_names(consequent, names, inside_function);
            if let Some(alternate) = alternate {
                collect_nested_function_names(alternate, names, inside_function);
            }
        }
        StatementKind::While { test, body } => {
            collect_identifiers(test, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::Labelled { body, .. } => {
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::With { object, body } => {
            collect_identifiers(object, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::ForIn { object, body, .. } => {
            collect_identifiers(object, names, inside_function);
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::DoWhile { body, test } => {
            collect_nested_function_names(body, names, inside_function);
            collect_identifiers(test, names, inside_function);
        }
        StatementKind::Switch {
            discriminant,
            cases,
        } => {
            collect_identifiers(discriminant, names, inside_function);
            for case in cases {
                if let Some(test) = &case.test {
                    collect_identifiers(test, names, inside_function);
                }
                for statement in &case.body {
                    collect_nested_function_names(statement, names, inside_function);
                }
            }
        }
        StatementKind::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(init) = init {
                collect_nested_function_names(init, names, inside_function);
            }
            if let Some(test) = test {
                collect_identifiers(test, names, inside_function);
            }
            if let Some(update) = update {
                collect_identifiers(update, names, inside_function);
            }
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::ForOf {
            pattern,
            iterable,
            body,
            ..
        } => {
            collect_identifiers(iterable, names, inside_function);
            // The loop's own pattern, for the reason a declaration's is walked above:
            // `for (const { a = n } of xs)` reads `n` where the loop is written.
            if inside_function {
                collect_pattern_identifiers(pattern, names);
            }
            collect_nested_function_names(body, names, inside_function);
        }
        StatementKind::Throw(value) => collect_identifiers(value, names, inside_function),
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            for statement in block.iter().chain(finalizer) {
                collect_nested_function_names(statement, names, inside_function);
            }
            if let Some(handler) = handler {
                for statement in &handler.body {
                    collect_nested_function_names(statement, names, inside_function);
                }
            }
        }
        StatementKind::Break { .. }
        | StatementKind::Continue { .. }
        | StatementKind::Empty
        | StatementKind::Unsupported { .. } => {}
    }
}

/// Walk an expression, recording identifiers when `collecting`.
///
/// It descends unconditionally and gates only the recording. That distinction matters: a function
/// *expression* nested in a `return` is a nested function whose free names are captures, and an
/// early return on `!collecting` would never reach it. That was a real bug -- `function f(n) {
/// return (m) => n + m; }` compiled with `n` unboxed, so the arrow could not resolve it.
/// Names a class's methods and field initialisers refer to.
///
/// Field initialisers count: they run inside the constructor, so a name one of them reads is
/// captured by the constructor exactly as a method body's would be.
fn collect_class_names(class: &Class, names: &mut HashSet<String>) {
    let bodies = class
        .constructor
        .iter()
        .chain(class.methods.iter().map(|method| &method.function))
        // A static block closes over the class's surroundings exactly as a method body does.
        .chain(
            class
                .initialisers
                .iter()
                .filter_map(ClassInitialiser::as_static_block),
        );
    for function in bodies {
        for parameter in &function.parameters {
            collect_pattern_identifiers(&parameter.pattern, names);
        }
        if let Some(rest) = &function.rest {
            collect_pattern_identifiers(rest, names);
        }
        for statement in &function.body {
            collect_nested_function_names(statement, names, true);
        }
    }
    for field in class
        .initialisers
        .iter()
        .filter_map(ClassInitialiser::as_field)
    {
        if let ObjectKey::Computed(key) = &field.key {
            collect_identifiers(key, names, true);
        }
        if let Some(value) = &field.value {
            collect_identifiers(value, names, true);
        }
    }
}

/// Every name a pattern binds.
/// The binary operation a compound assignment performs before storing.
fn compound_operator(operator: AssignmentOperator) -> BinaryOp {
    match operator {
        AssignmentOperator::Add => BinaryOp::Add,
        AssignmentOperator::Subtract => BinaryOp::Subtract,
        AssignmentOperator::Multiply => BinaryOp::Multiply,
        AssignmentOperator::Divide => BinaryOp::Divide,
        AssignmentOperator::Remainder => BinaryOp::Remainder,
        AssignmentOperator::Exponentiate => BinaryOp::Exponentiate,
        AssignmentOperator::BitwiseAnd => BinaryOp::BitwiseAnd,
        AssignmentOperator::BitwiseOr => BinaryOp::BitwiseOr,
        AssignmentOperator::BitwiseXor => BinaryOp::BitwiseXor,
        AssignmentOperator::ShiftLeft => BinaryOp::ShiftLeft,
        AssignmentOperator::ShiftRight => BinaryOp::ShiftRight,
        AssignmentOperator::UnsignedShiftRight => BinaryOp::UnsignedShiftRight,
        AssignmentOperator::Assign => unreachable!("a plain assignment performs no operation"),
        AssignmentOperator::LogicalAnd
        | AssignmentOperator::LogicalOr
        | AssignmentOperator::Nullish => {
            unreachable!("a short-circuiting assignment computes nothing")
        }
    }
}

/// The jump that skips the store for a short-circuiting assignment.
///
/// `a ||= b` assigns only when `a` is falsy, so the jump taken *past* the store is the one taken
/// when `a` is truthy -- which is why each operator maps to the opposite test.
fn short_circuit_jump(operator: AssignmentOperator) -> Instruction {
    match operator {
        AssignmentOperator::LogicalAnd => Instruction::JumpIfFalse(0),
        AssignmentOperator::LogicalOr => Instruction::JumpIfTruthy(0),
        AssignmentOperator::Nullish => Instruction::JumpIfNotNullish(0),
        _ => unreachable!("only a short-circuiting assignment skips its store"),
    }
}

/// The kind an installed private element carries, from the syntax that declared it.
fn private_kind(kind: kiln_hir::MethodKind) -> PrivateKind {
    match kind {
        kiln_hir::MethodKind::Normal => PrivateKind::Method,
        kiln_hir::MethodKind::Get => PrivateKind::Getter,
        kiln_hir::MethodKind::Set => PrivateKind::Setter,
    }
}

/// The reserved property name a private member is stored under.
///
/// Reserved rather than ordinary, so nothing that enumerates own string properties can see it and
/// no computed access can spell it -- `obj["#x"]` names a different property, as it should.
fn private_key(scope: u32, name: &str) -> String {
    format!("@@private:{scope}:{name}")
}

/// The names a pattern binds, in source order.
///
/// Ordered, unlike [`collect_pattern_names`], because these decide slot numbering and the
/// bytecode is a cache key: the same source has to produce the same instructions, and a hash set
/// does not promise that.
fn pattern_names_in_order(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Identifier(name) => names.push(name.clone()),
        // A reference target binds nothing: `[o.p] = xs` stores into an object that already
        // exists, and `o` was declared somewhere else.
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::WithDefault { inner, .. } => pattern_names_in_order(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                pattern_names_in_order(&property.value, names);
            }
            if let Some(rest) = rest {
                pattern_names_in_order(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                pattern_names_in_order(element, names);
            }
            if let Some(rest) = rest {
                pattern_names_in_order(rest, names);
            }
        }
    }
}

fn collect_pattern_names(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier(name) => {
            names.insert(name.clone());
        }
        Pattern::Member { .. } | Pattern::Computed { .. } | Pattern::PrivateMember { .. } => {}
        Pattern::WithDefault { inner, .. } => collect_pattern_names(inner, names),
        Pattern::Object { properties, rest } => {
            for property in properties {
                collect_pattern_names(&property.value, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
        Pattern::Array { elements, rest } => {
            for element in elements.iter().flatten() {
                collect_pattern_names(element, names);
            }
            if let Some(rest) = rest {
                collect_pattern_names(rest, names);
            }
        }
    }
}

fn collect_identifiers(expression: &Expression, names: &mut HashSet<String>, collecting: bool) {
    match &expression.kind {
        // A nested function's body is always collected, whatever the enclosing flag says.
        ExpressionKind::Function(function) => {
            for parameter in &function.parameters {
                collect_pattern_identifiers(&parameter.pattern, names);
            }
            if let Some(rest) = &function.rest {
                collect_pattern_identifiers(rest, names);
            }
            for statement in &function.body {
                // A function expression always establishes a nested function relative to the
                // expression that contains it. This must not inherit `collecting`: at module
                // level that flag is false by design, yet
                // `Layer.prototype.match = function match () { return MODULE_CONST }` still
                // closes over the module. Inheriting false emitted a `LoadGlobal` that later
                // indexed whichever CommonJS module called the exported method.
                collect_nested_function_names(statement, names, true);
            }
        }
        // A class expression's methods and field initialisers are nested functions too, and a
        // field initialiser closing over the enclosing scope is exactly what g03 exercises.
        ExpressionKind::Class(class) => {
            collect_class_names(class, names);
        }
        ExpressionKind::Identifier(name) => {
            if collecting {
                names.insert(name.clone());
            }
        }
        ExpressionKind::Update { target, .. } => {
            if collecting {
                names.insert(target.clone());
            }
        }
        ExpressionKind::PrivateIn { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
        ExpressionKind::SuperComputedMember { property } => {
            collect_identifiers(property, names, collecting);
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::This
        | ExpressionKind::NewTarget
        | ExpressionKind::SuperMember { .. }
        | ExpressionKind::Unsupported { .. } => {}
        ExpressionKind::SuperCall { arguments } => {
            for argument in arguments {
                let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::Unary { argument, .. } => collect_identifiers(argument, names, collecting),
        ExpressionKind::Binary { left, right, .. }
        | ExpressionKind::Logical { left, right, .. } => {
            collect_identifiers(left, names, collecting);
            collect_identifiers(right, names, collecting);
        }
        ExpressionKind::Assignment { target, value, .. } => {
            if collecting {
                names.insert(target.clone());
            }
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::ImportCall { specifier, options } => {
            collect_identifiers(specifier, names, collecting);
            if let Some(options) = options {
                collect_identifiers(options, names, collecting);
            }
        }
        // Every name the pattern assigns is written by this function, so each is a capture
        // candidate for the same reason a plain assignment target is.
        ExpressionKind::PatternAssignment { pattern, value } => {
            if collecting {
                collect_pattern_names(pattern, names);
            }
            collect_pattern_captures(pattern, names);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Conditional {
            test,
            consequent,
            alternate,
        } => {
            collect_identifiers(test, names, collecting);
            collect_identifiers(consequent, names, collecting);
            collect_identifiers(alternate, names, collecting);
        }
        ExpressionKind::Call {
            callee, arguments, ..
        }
        | ExpressionKind::New { callee, arguments } => {
            collect_identifiers(callee, names, collecting);
            for argument in arguments {
                let (ListElement::Item(value) | ListElement::Spread(value)) = argument else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::Array { elements } => {
            for element in elements {
                let (ListElement::Item(value) | ListElement::Spread(value)) = element else {
                    continue;
                };
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::Sequence { operands } => {
            for operand in operands {
                collect_identifiers(operand, names, collecting);
            }
        }
        ExpressionKind::Yield { value, .. } => {
            if let Some(value) = value {
                collect_identifiers(value, names, collecting);
            }
        }
        ExpressionKind::ComputedMember {
            object, property, ..
        } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(property, names, collecting);
        }
        ExpressionKind::ComputedAssignment {
            object,
            property,
            value,
            ..
        } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(property, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::MemberAssignment { object, value, .. } => {
            collect_identifiers(object, names, collecting);
            // The value is walked like any other expression: a function expression in it
            // establishes its own nesting (the `Function` arm above collects its body whatever
            // this flag says, which is what keeps `Layer.prototype.match = function match (...)
            // {}` closing over its module), and a plain value does not. Walking the value as if
            // it were nested boxed every local a member write read -- `m.h = m.f` made `m` a
            // cell -- and a cell is a heap read at every use and an escape to the scalar
            // analysis.
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::SuperMemberAssignment { value, .. } => {
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::SuperComputedAssignment {
            property, value, ..
        } => {
            collect_identifiers(property, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::Chain { expression } | ExpressionKind::Await(expression) => {
            collect_identifiers(expression, names, collecting);
        }
        ExpressionKind::RegExp { .. } => {}
        ExpressionKind::PrivateMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
        ExpressionKind::PrivateAssignment { object, value, .. } => {
            collect_identifiers(object, names, collecting);
            collect_identifiers(value, names, collecting);
        }
        ExpressionKind::UpdateMember {
            object, property, ..
        } => {
            collect_identifiers(object, names, collecting);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_identifiers(key, names, collecting);
            }
        }
        // A template object is built from constants; it names nothing.
        ExpressionKind::TemplateStrings { .. } => {}
        ExpressionKind::Template { expressions, .. } => {
            for expression in expressions {
                collect_identifiers(expression, names, collecting);
            }
        }
        ExpressionKind::Delete { object, property } => {
            collect_identifiers(object, names, collecting);
            if let ObjectKey::Computed(key) = property.as_ref() {
                collect_identifiers(key, names, collecting);
            }
        }
        ExpressionKind::DeleteValue { argument } => {
            collect_identifiers(argument, names, collecting);
        }
        // The name is a reference like any other: an eval that writes `delete x` needs the outer
        // `x` bound, or it would ask the global object about a binding a caller's scope holds.
        ExpressionKind::DeleteBinding { name } => {
            if collecting {
                names.insert(name.clone());
            }
        }
        ExpressionKind::Object { members } => {
            for member in members {
                match member {
                    ObjectMember::Property(property) => {
                        if let ObjectKey::Computed(key) = &property.key {
                            collect_identifiers(key, names, collecting);
                        }
                        // Property values are expressions in this lexical region. For capture
                        // analysis the important case is a function value: its body is nested
                        // relative to the current owner even when the surrounding expression is
                        // itself already inside another function.
                        collect_identifiers(&property.value, names, collecting);
                    }
                    ObjectMember::Spread(source) => {
                        collect_identifiers(source, names, collecting);
                    }
                    ObjectMember::Accessor { key, function, .. } => {
                        if let ObjectKey::Computed(key) = key {
                            collect_identifiers(key, names, collecting);
                        }
                        // The body is a nested function, so every name in it is a candidate.
                        for statement in &function.body {
                            collect_nested_function_names(statement, names, true);
                        }
                    }
                }
            }
        }
        ExpressionKind::StaticMember { object, .. } => {
            collect_identifiers(object, names, collecting);
        }
    }
}
/// The block-level function declarations Annex B gives a var-scoped binding, in source order.
///
/// `nested` says whether this list is one the specification calls a `Block`, `CaseClause` or
/// `DefaultClause` rather than the top level of the variable environment: a declaration written
/// at the top level is already var-scoped by the main grammar and is not Annex B's business.
///
/// `blocked` is every name a lexical declaration between the variable environment and here has
/// taken, because replacing the function declaration with a `var` of that name would be an early
/// error and Annex B declines exactly when it would be.
fn collect_annexb_function_names(
    body: &[Statement],
    nested: bool,
    blocked: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    let depth = blocked.len();
    for statement in body {
        collect_lexical_names(statement, blocked);
    }
    for statement in body {
        if nested
            && let StatementKind::Function(function) = &statement.kind
            && let Some(name) = &function.name
            // Only a plain function declaration. A generator or an async function inside a block
            // is block-scoped and nothing more; Annex B never mentions either.
            && !function.generator
            && !function.asynchronous
            && !blocked.contains(name)
            && !out.contains(name)
        {
            out.push(name.clone());
        }
        collect_annexb_in_statement(statement, blocked, out);
    }
    blocked.truncate(depth);
}

/// Walk one statement for the nested lists Annex B looks inside, without leaving this variable
/// environment: a nested function owns its own, and so does a class body.
fn collect_annexb_in_statement(
    statement: &Statement,
    blocked: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    match &statement.kind {
        StatementKind::Block(body) => collect_annexb_function_names(body, true, blocked, out),
        StatementKind::If {
            consequent,
            alternate,
            ..
        } => {
            collect_annexb_branch(consequent, blocked, out);
            if let Some(alternate) = alternate {
                collect_annexb_branch(alternate, blocked, out);
            }
        }
        StatementKind::While { body, .. } | StatementKind::DoWhile { body, .. } => {
            collect_annexb_in_statement(body, blocked, out);
        }
        // A label does not open a scope, so a declaration under one is at whatever level the
        // label is: `l: function f() {}` at the top of a body is var-scoped already.
        StatementKind::Labelled { body, .. } => collect_annexb_in_statement(body, blocked, out),
        StatementKind::With { body, .. } => collect_annexb_in_statement(body, blocked, out),
        StatementKind::For { init, body, .. } => {
            let depth = blocked.len();
            if let Some(init) = init {
                collect_lexical_names(init, blocked);
            }
            collect_annexb_in_statement(body, blocked, out);
            blocked.truncate(depth);
        }
        StatementKind::ForIn {
            kind, name, body, ..
        } => {
            let depth = blocked.len();
            if *kind != VariableKind::Var {
                blocked.push(name.clone());
            }
            collect_annexb_in_statement(body, blocked, out);
            blocked.truncate(depth);
        }
        StatementKind::ForOf {
            kind,
            pattern,
            body,
            ..
        } => {
            let depth = blocked.len();
            if *kind != VariableKind::Var {
                pattern_names_in_order(pattern, blocked);
            }
            collect_annexb_in_statement(body, blocked, out);
            blocked.truncate(depth);
        }
        // Every clause of a `switch` shares one lexical scope, so a `let` in any of them blocks
        // a function declaration in all of them.
        StatementKind::Switch { cases, .. } => {
            let depth = blocked.len();
            for case in cases {
                for statement in &case.body {
                    collect_lexical_names(statement, blocked);
                }
            }
            for case in cases {
                for statement in &case.body {
                    if let StatementKind::Function(function) = &statement.kind
                        && let Some(name) = &function.name
                        && !function.generator
                        && !function.asynchronous
                        && !blocked.contains(name)
                        && !out.contains(name)
                    {
                        out.push(name.clone());
                    }
                    collect_annexb_in_statement(statement, blocked, out);
                }
            }
            blocked.truncate(depth);
        }
        StatementKind::Try {
            block,
            handler,
            finalizer,
        } => {
            collect_annexb_function_names(block, true, blocked, out);
            if let Some(handler) = handler {
                // B.3.4 removes the early error for `catch (e) { var e; }`, so a catch parameter
                // does not block -- and the HIR carries only the simple form, which is exactly
                // the one the exemption is written for.
                collect_annexb_function_names(&handler.body, true, blocked, out);
            }
            collect_annexb_function_names(finalizer, true, blocked, out);
        }
        _ => {}
    }
}

/// An `if` branch is a statement rather than a list, and a function declaration written directly
/// as one is the form Annex B B.3.4 exists for: `if (x) function f() {}`.
fn collect_annexb_branch(statement: &Statement, blocked: &mut Vec<String>, out: &mut Vec<String>) {
    if let StatementKind::Function(function) = &statement.kind
        && let Some(name) = &function.name
        && !function.generator
        && !function.asynchronous
        && !blocked.contains(name)
        && !out.contains(name)
    {
        out.push(name.clone());
    }
    collect_annexb_in_statement(statement, blocked, out);
}

/// The names one statement declares lexically, which are the ones a `var` of the same name could
/// not join.
fn collect_lexical_names(statement: &Statement, names: &mut Vec<String>) {
    match &statement.kind {
        StatementKind::Variable { kind, declarations } if *kind != VariableKind::Var => {
            for declaration in declarations {
                pattern_names_in_order(&declaration.pattern, names);
            }
        }
        StatementKind::Class(class) => {
            if let Some(name) = &class.name {
                names.push(name.clone());
            }
        }
        _ => {}
    }
}

/// Whether a property's value is a *syntactically* anonymous function definition.
///
/// `NamedEvaluation` names one of these after the property key; it does not name a reference to
/// a function that happens to have no name, which is why this asks the shape of the expression
/// rather than the name of the value. `{ [k]: (0, function () {}) }` is a parenthesised
/// reference and keeps the empty name. An arrow is a `Function` with the flag set and is always
/// anonymous, so it needs no arm of its own.
fn anonymous_function_value(value: &kiln_hir::Expression) -> bool {
    match &value.kind {
        kiln_hir::ExpressionKind::Function(function) => function.name.is_none(),
        kiln_hir::ExpressionKind::Class(class) => class.name.is_none(),
        _ => false,
    }
}

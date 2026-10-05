use super::*;

pub(super) fn realm_runtime() -> Result<RealmRuntime, ExecutionError> {
    RealmRuntime::new().map_err(|error| ExecutionError::Host {
        instruction: 0,
        message: error.to_string(),
    })
}

fn initialize_host_clock<H: HostOps + ?Sized>(
    runtime: &mut RealmRuntime,
    host: &mut H,
) -> Result<(), ExecutionError> {
    if runtime.capabilities().clock == ClockCapability::Host && !runtime.has_host_clock_origin() {
        let epoch_ms = crate::vm_setup::host_clock_milliseconds(host, 0)?;
        runtime.initialize_host_clock_origin(epoch_ms);
    }
    Ok(())
}

pub(super) fn realm_host_string<H: HostOps + ?Sized>(
    host: &mut H,
    operation: HostOp,
    label: &str,
) -> Result<String, ExecutionError> {
    match host
        .call(operation, &[])
        .map_err(|error| ExecutionError::Host {
            instruction: 0,
            message: error.to_string(),
        })? {
        HostValue::String(value) if !value.is_empty() => Ok(value.to_lossy_string()),
        _ => Err(ExecutionError::Host {
            instruction: 0,
            message: format!("HostOp::{label} must return a non-empty string"),
        }),
    }
}

pub(super) fn resolve_timezone<H: HostOps + ?Sized>(
    host: &mut H,
    capability: &TimeZoneCapability,
) -> Result<String, ExecutionError> {
    let value = match capability {
        TimeZoneCapability::Utc => "UTC".to_owned(),
        TimeZoneCapability::Named(value) => value.clone(),
        TimeZoneCapability::Host => realm_host_string(host, HostOp::TimeZone, "TimeZone")?,
    };
    if kiln_intl::datetime::canonical_time_zone(&value).is_none() {
        return Err(ExecutionError::Host {
            instruction: 0,
            message: format!("invalid realm timezone: {value}"),
        });
    }
    Ok(value)
}

pub(super) fn resolve_locale<H: HostOps + ?Sized>(
    host: &mut H,
    capability: &LocaleCapability,
) -> Result<String, ExecutionError> {
    let value = match capability {
        LocaleCapability::Deterministic => kiln_intl::locale::DEFAULT.to_owned(),
        LocaleCapability::Named(value) => value.clone(),
        LocaleCapability::Host => realm_host_string(host, HostOp::Locale, "Locale")?,
    };
    kiln_intl::locale::canonicalize(&value).map_err(|error| ExecutionError::Host {
        instruction: 0,
        message: format!("invalid realm locale: {error}"),
    })
}

/// Execute one IR program against a host implementation.
pub fn execute<H: HostOps + ?Sized>(
    program: Rc<Program>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<ExecutionResult, ExecutionError> {
    let strictness = Rc::new(Strictness::sloppy(&program));
    execute_with_strictness(program, strictness, host, limits, capabilities)
}

/// [`execute`], told which of the program's bodies the compiler compiled as strict code.
///
/// The strictness table is not in the bytecode -- see [`Strictness`] -- so a caller that has one
/// has to hand it over, and a caller that does not gets sloppy mode, where a refused write is
/// discarded rather than raised.
pub fn execute_with_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<ExecutionResult, ExecutionError> {
    let mut runtime = realm_runtime()?;
    execute_in_runtime_with_strictness(
        program,
        strictness,
        host,
        limits,
        &mut runtime,
        capabilities,
    )
}

/// Execute one script under a caller-owned realm runtime.
///
/// This is ADR 0031's policy injection seam: the ordinary [`execute`] API keeps the deterministic
/// default, while embedders can construct a runtime with explicit capabilities without adding a
/// parallel scheduler or letting the interpreter discover ambient state.
pub fn execute_in_runtime_with_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
) -> Result<ExecutionResult, ExecutionError> {
    initialize_host_clock(runtime, host)?;
    let locals = vec![Value::Undefined; program.local_count as usize];
    let mut vm = Vm::with_entropy_seed(
        program,
        strictness,
        host,
        locals,
        limits,
        runtime.capabilities(),
        capabilities,
    )?;
    vm.host_clock_origin_ms = (runtime.capabilities().clock == ClockCapability::Host)
        .then(|| runtime.host_clock_anchor());
    // The HTTP runtime is available for global fetch (a WHATWG web standard, not a Node API).
    // The realm's HTTP transport (Hyper via Tokio) exists in both modes; only the Node.js
    // API surface (require, process, etc.) remains gated behind the node adapter.
    // The http_client_host allows the HTTP client to establish TCP connections without
    // the full async_host (which would also enable node:fs, node:net, etc.).
    vm.http_runtime = Some(runtime as *mut RealmRuntime as *mut dyn ActiveHttpRuntime);
    vm.http_client_host = Some(runtime as *mut RealmRuntime as *mut dyn ActiveAsyncHostOps);
    // A Script's `this` is the global object, and in strict code too: strictness governs what a
    // *function* receives when called without a receiver, not what a script body's `this` is.
    // Only a module gets `undefined`, and `execute_module` builds its own machine -- the comment
    // on `Vm::this_value` says exactly that, and this path was reading it as though every program
    // were a module. Setting it here also forces the realm's global object into existence before
    // the program runs, which is what makes `this.x = 1` and a later bare `x` name one binding --
    // and runs the realm bootstrap, so the program never sees a realm without its built-ins.
    // The program is not finished when its last statement is: anything it scheduled still has
    // to run, which is what makes a `setTimeout` observable at all.
    let execution = runtime.enter(|| {
        vm.this_value = vm.realize_realm(RealmId::PRIMARY)?;
        vm.run()
    })?;
    vm.run_event_loop(runtime)?;
    Ok(ExecutionResult {
        steps: vm.steps,
        process_exit_code: vm.process_exit_code(None),
        ..execution
    })
}

/// Execute one program with the Tokio-owned Node host Module installed.
///
/// This is intentionally opt-in: deterministic Test262 and build-time evaluation cannot observe
/// ambient filesystem state, while a Node-compatible CLI run explicitly can. Both modes retain
/// the same language queues and Promise implementation; only this Adapter supplies live I/O.
pub fn execute_node<H: HostOps + ?Sized>(
    program: Rc<Program>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<ExecutionResult, ExecutionError> {
    let strictness = Rc::new(Strictness::sloppy(&program));
    execute_node_with_strictness(program, strictness, host, limits, capabilities)
}

/// [`execute_node`], told which of the program's bodies are strict code.
pub fn execute_node_with_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<ExecutionResult, ExecutionError> {
    execute_node_with_clock_and_strictness(
        program,
        strictness,
        host,
        limits,
        capabilities,
        ClockCapability::Virtual,
    )
}

/// Execute Node code under an explicit clock policy. Deterministic callers use the ordinary
/// `execute_node_with_strictness` entry point; wall-clock workloads opt in here.
pub fn execute_node_with_clock_and_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
    clock: ClockCapability,
) -> Result<ExecutionResult, ExecutionError> {
    let mut runtime = RealmRuntime::with_capabilities(RealmCapabilities {
        clock,
        ..RealmCapabilities::default()
    })
    .map_err(|error| ExecutionError::Host {
        instruction: 0,
        message: error.to_string(),
    })?;
    execute_node_in_runtime_with_strictness(
        program,
        strictness,
        host,
        limits,
        &mut runtime,
        capabilities,
    )
}

/// Execute one Node script under a caller-owned realm runtime and capability policy.
pub fn execute_node_in_runtime_with_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
) -> Result<ExecutionResult, ExecutionError> {
    if let Some(fs) = host.clone_fs() {
        runtime.set_fs(fs);
    }
    initialize_host_clock(runtime, host)?;
    let locals = vec![Value::Undefined; program.local_count as usize];
    let mut vm = Vm::with_entropy_seed(
        program,
        strictness,
        host,
        locals,
        limits,
        runtime.capabilities(),
        capabilities,
    )?;
    vm.host_clock_origin_ms = (runtime.capabilities().clock == ClockCapability::Host)
        .then(|| runtime.host_clock_anchor());
    vm.async_host = Some(runtime as *mut RealmRuntime as *mut dyn ActiveAsyncHostOps);
    vm.http_runtime = Some(runtime as *mut RealmRuntime as *mut dyn ActiveHttpRuntime);
    let execution = runtime.enter(|| {
        vm.realize_realm(RealmId::PRIMARY)?;
        vm.run()
    })?;
    vm.run_node_event_loop(runtime)?;
    Ok(ExecutionResult {
        steps: vm.steps,
        process_exit_code: vm.process_exit_code(None),
        ..execution
    })
}

/// Run a program whose values are all discarded, reclaiming the cycles it created.
///
/// For an embedder that runs many independent programs in one process -- the Test262 harness is
/// the one that needs this -- where [`execute`] would leave every function of every program
/// allocated for the life of the process. See [`Vm::release_cycles`] for why that happens.
///
/// Only the step count comes back, because a `Value` is exactly what is *not* safe to keep: the
/// functions reachable from one have had their links cut by the time this returns. An embedder
/// that needs the result value wants [`execute`] and the leak that comes with it.
pub fn execute_reclaiming<H: HostOps + ?Sized>(
    program: Rc<Program>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<u64, ExecutionError> {
    let strictness = Rc::new(Strictness::sloppy(&program));
    execute_reclaiming_with_strictness(program, strictness, host, limits, capabilities)
}

/// [`execute_reclaiming`], told which of the program's bodies are strict code.
pub fn execute_reclaiming_with_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<u64, ExecutionError> {
    let mut runtime = realm_runtime()?;
    execute_reclaiming_in_runtime_with_strictness(
        program,
        strictness,
        host,
        limits,
        &mut runtime,
        capabilities,
    )
}

/// Execute and reclaim one independent script on a caller-owned runtime.
///
/// The runtime owns the expensive current-thread Tokio scheduler, not JavaScript globals. Every
/// call still creates a fresh VM, language realm, global object, locals, and entropy stream. This
/// is the persistent-worker seam for embedders such as the conformance coordinator: they can load
/// scheduler infrastructure once while preserving the same per-case isolation as
/// [`execute_reclaiming_with_strictness`].
///
/// Callers must not reuse a runtime after [`RealmRuntime::shutdown`]. Node execution is excluded
/// deliberately: a Node realm can retain provider resources whose reset policy is a separate
/// lifecycle contract.
pub fn execute_reclaiming_in_runtime_with_strictness<H: HostOps + ?Sized>(
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
) -> Result<u64, ExecutionError> {
    let locals = vec![Value::Undefined; program.local_count as usize];
    let mut vm = Vm::with_entropy_seed(
        program,
        strictness,
        host,
        locals,
        limits,
        runtime.capabilities(),
        capabilities,
    )?;
    // A Script's `this` is the global object, and in strict code too: strictness governs what a
    // *function* receives when called without a receiver, not what a script body's `this` is.
    // Only a module gets `undefined`, and `execute_module` builds its own machine -- the comment
    // on `Vm::this_value` says exactly that, and this path was reading it as though every program
    // were a module. Setting it here also forces the realm's global object into existence before
    // the program runs, which is what makes `this.x = 1` and a later bare `x` name one binding --
    // and runs the realm bootstrap, so the program never sees a realm without its built-ins.
    let outcome = runtime
        .enter(|| {
            vm.this_value = vm.realize_realm(RealmId::PRIMARY)?;
            vm.run()
        })
        .and_then(|_| vm.run_event_loop(runtime));
    // Unconditionally, and before the `?`: a program that threw still defined every function it
    // evaluated on the way to throwing, and those cycles are no less permanent.
    vm.release_cycles();
    outcome?;
    Ok(vm.steps)
}

/// Whether this value's `[[Set]]` is defined to refuse, whatever its properties' flags say.
///
/// One predicate rather than a check in each of the two spellings of a write, because the two
/// have to agree: `ns.x = 1` throws in strict code exactly when `Reflect.set(ns, "x", 1)`
/// answers false, and a module namespace is where the flags cannot be asked -- its exports
/// report `writable: true` so that a program reading a descriptor is told the truth, while the
/// write never lands.
pub(super) fn refuses_every_write(target: &Value) -> bool {
    matches!(target, Value::Object(object) if object.borrow().is_module_namespace())
}

/// The object `import * as ns` binds: one property per export of the provider.
///
/// Built by the linker rather than by the module itself, because the module does not know it is
/// being imported this way. Frozen, which is what a module namespace is -- assigning to one is a
/// TypeError in the specification and silently ignored here, as every frozen object's writes are.
pub fn namespace_object(exports: impl IntoIterator<Item = (String, Value)>) -> Value {
    let mut data = ObjectData::default();
    let mut namespace_cells = Vec::new();
    for (name, value) in exports {
        // A namespace object holds values, so an exported cell is read through here. Node's
        // namespace objects are live; this one is a snapshot taken when the provider finished.
        //
        // Writable and non-configurable, which is the one combination no ordinary object is
        // built with and the one the specification gives every export: the binding cannot be
        // removed or redefined, and its *value* is expected to change, because the module it
        // came from may still assign to it.
        data.define_value(
            &name,
            read_through_cell(&value),
            Attributes {
                writable: true,
                enumerable: true,
                configurable: false,
            },
        );
        if matches!(value, Value::Cell(_)) {
            namespace_cells.push((name, value));
        }
    }
    // The tag is what `Object.prototype.toString` reads, and it is the only own property a
    // namespace has that is not an export -- so `String(ns)` is "[object Module]" rather than
    // "[object Object]", which is how a program tells a namespace from an object of exports.
    data.define_value(
        WellKnown::ToStringTag.property_key(),
        Value::String("Module".to_owned().into()),
        Attributes {
            writable: false,
            enumerable: false,
            configurable: false,
        },
    );
    // A namespace inherits from nothing. `ns.toString` is `undefined`, `ns.hasOwnProperty` is
    // `undefined`, and a name a module did not export is absent rather than answered by
    // `Object.prototype` -- which is what stops an importer from reading someone else's
    // furniture through a module it imported.
    data.set_prototype(Value::Null);
    data.close_as_module_namespace();
    // Finish defining the whole shape before recording indices: array-index export names can
    // be inserted ahead of earlier string exports while the property store maintains its order.
    for (name, cell) in namespace_cells {
        data.track_namespace_cell(&name, cell);
    }
    Value::Object(tracked_object(data))
}

/// Reading an export whose declaration has not run yet.
///
/// Worded as the language words it for a dead-zone read of the binding itself, because that is
/// what it is: the namespace is a second name for the same binding, not a copy of it.
pub(super) fn uninitialized_export_error(name: &str) -> ExecutionError {
    ExecutionError::ReferenceErrorMessage {
        message: format!("Cannot access '{name}' before initialization"),
    }
}

/// Whether this generator object came from an `async function*`.
pub(super) fn is_async_generator(state: &Rc<GeneratorData>) -> bool {
    let state = state.borrow();
    state
        .closure
        .program
        .functions
        .get(state.closure.function as usize)
        .is_some_and(|proto| proto.async_generator)
}

/// What a value is, looking through an exported cell.
///
/// An export is a cell so that a named import is a live view of it. Everywhere a *value* is
/// wanted instead -- a namespace object, a star expansion -- goes through this.
pub fn read_through_cell(value: &Value) -> Value {
    let mut current = value.clone();
    // Brent's cycle detector keeps the ordinary one-cell import read allocation-free. A
    // pathological alias cycle retains its TDZ answer rather than spinning forever.
    let mut checkpoint = None;
    let mut window = 1usize;
    let mut distance = 0usize;
    while let Value::Cell(cell) = current {
        let address = Rc::as_ptr(&cell);
        if checkpoint == Some(address) {
            return Value::Native(Native::Uninitialized, RealmId::PRIMARY);
        }
        if checkpoint.is_none() {
            checkpoint = Some(address);
        } else if distance == window {
            checkpoint = Some(address);
            window = window.saturating_mul(2);
            distance = 0;
        }
        current = cell.borrow().clone();
        distance = distance.saturating_add(1);
    }
    current
}

/// The own properties of a namespace object, in the order it holds them.
///
/// The inverse of [`namespace_object`], for the linker expanding a `export * from` entry back
/// into the names it stands for.
pub fn namespace_entries(value: &Value) -> Vec<(String, Value)> {
    let Value::Object(object) = value else {
        return Vec::new();
    };
    object
        .borrow()
        .entries()
        .filter_map(|(name, property)| match property {
            // The tag is the namespace's own furniture, not one of the module's exports. A star
            // re-export expanded through here would otherwise hand `@@toStringTag` on to the
            // importer as a name the provider had exported.
            _ if name == WellKnown::ToStringTag.property_key() => None,
            Property::Value(value) => Some((name.to_lossy_string(), value.clone())),
            Property::Accessor { .. } => None,
        })
        .collect()
}

/// Construct an ordinary mutable object for a host-defined module wrapper.
pub fn ordinary_object(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    let mut data = ObjectData::default();
    for (name, value) in entries {
        data.set(&name, value);
    }
    Value::Object(tracked_object(data))
}

/// Read one own data property from an interpreter object without invoking program code.
pub fn own_data_property(value: &Value, name: &str) -> Option<Value> {
    let Value::Object(object) = value else {
        return None;
    };
    object.borrow().get_own(name).cloned()
}

/// What `node:worker_threads` reads about the exception that ended a worker (realms issue 06):
/// the tokens its parent's `error` event decodes, in the message layout of
/// `kiln-builtins/src/web_events_streams.ts` -- no transferred ports, then the value. An error is
/// its name, message and stack; any other value is itself when it is a primitive, and an empty
/// object otherwise.
pub fn worker_error_tokens(
    thrown: &Value,
    error_constructor: Option<&str>,
) -> Vec<kiln_runtime::workers::Token> {
    use kiln_runtime::workers::Token;
    const T_UNDEFINED: f64 = 0.0;
    const T_NULL: f64 = 1.0;
    const T_FALSE: f64 = 2.0;
    const T_TRUE: f64 = 3.0;
    const T_NUMBER: f64 = 4.0;
    const T_STRING: f64 = 5.0;
    const T_BIGINT: f64 = 6.0;
    const T_OBJECT: f64 = 8.0;
    const T_ERROR: f64 = 19.0;
    let mut out = vec![Token::Number(0.0)];
    match thrown {
        Value::Undefined => out.push(Token::Number(T_UNDEFINED)),
        Value::Null => out.push(Token::Number(T_NULL)),
        Value::Boolean(value) => out.push(Token::Number(if *value { T_TRUE } else { T_FALSE })),
        Value::Number(value) => out.extend([Token::Number(T_NUMBER), Token::Number(*value)]),
        Value::String(text) => out.extend([Token::Number(T_STRING), Token::String(text.clone())]),
        Value::BigInt(value) => out.extend([
            Token::Number(T_BIGINT),
            Token::String(Value::BigInt(value.clone()).to_display_string().into()),
        ]),
        Value::Object(object) => {
            let text = |name: &str| -> Option<StringUnits> {
                let mut current = Some(Rc::clone(object));
                for _ in 0..64 {
                    let here = current?;
                    let borrowed = here.borrow();
                    if let Some(Value::String(text)) = borrowed.get_own(name) {
                        return Some(text.clone());
                    }
                    current = match &borrowed.prototype {
                        Some(Value::Object(next)) => Some(Rc::clone(next)),
                        _ => None,
                    };
                }
                None
            };
            let message = object.borrow().get_own("message").cloned();
            let is_error = error_constructor.is_some_and(|name| name.ends_with("Error"))
                || matches!(message, Some(Value::String(_)));
            if !is_error {
                out.extend([Token::Number(T_OBJECT), Token::Number(0.0)]);
                return out;
            }
            let name = text("name")
                .or_else(|| error_constructor.map(StringUnits::from))
                .unwrap_or_else(|| StringUnits::from("Error"));
            let message = match message {
                Some(Value::String(text)) => text,
                _ => StringUnits::from(""),
            };
            out.extend([
                Token::Number(T_ERROR),
                Token::String(name),
                Token::String(message),
            ]);
            match object.borrow().get_own("stack") {
                Some(Value::String(stack)) => {
                    out.extend([Token::Number(T_STRING), Token::String(stack.clone())])
                }
                _ => out.push(Token::Number(T_UNDEFINED)),
            }
            out.push(Token::Number(0.0));
        }
        _ => out.extend([Token::Number(T_OBJECT), Token::Number(0.0)]),
    }
    out
}

/// Parse a JSON module into the same JavaScript value graph used by `JSON.parse`.
///
/// Module loaders use this instead of translating through a second JSON representation, keeping
/// number semantics, insertion order, duplicate-key behavior, and object identity identical to
/// language-level parsing.
pub fn parse_json_module(source: &str) -> Result<Value, ExecutionError> {
    let source = StringUnits::from(source);
    parse_json_text(&source)
}

/// Execute one linkage-aware module after its acyclic dependencies have completed.
///
/// `import_values` is ordered exactly like [`ModuleProgram::imports`]. Each value is installed in
/// the compiler-assigned local slot before the first bytecode instruction executes.
pub fn execute_module<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    capabilities: ExecutionCapabilities,
) -> Result<ModuleExecutionResult, ExecutionError> {
    let mut runtime = realm_runtime()?;
    execute_module_in_runtime(
        module,
        import_values,
        host,
        limits,
        &mut runtime,
        capabilities,
    )
}

/// Execute one linked module under a caller-owned Realm Runtime.
///
/// A module graph uses this form so all module bodies enter one Tokio runtime rather than paying
/// for and shutting down an executor per module. The language Realm itself is still scoped to the
/// module VM in this migration slice; graph-wide queues move out with the native continuation ABI.
pub fn execute_module_in_runtime<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
) -> Result<ModuleExecutionResult, ExecutionError> {
    execute_module_in_runtime_mode(
        module,
        import_values,
        host,
        limits,
        runtime,
        capabilities,
        false,
    )
}

/// Execute one module while allowing graph-owned CommonJS targets to load on demand.
#[allow(clippy::too_many_arguments)]
pub fn execute_module_in_runtime_with_loader<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    self_imports: &[ModuleSelfImport],
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
    loader: &mut dyn CommonJsModuleLoader,
    enter_runtime: bool,
    module_path: Option<&str>,
) -> Result<ModuleExecutionResult, ExecutionError> {
    execute_module_in_runtime_mode_with_session(
        module,
        import_values,
        self_imports,
        host,
        limits,
        runtime,
        capabilities,
        None,
        Some(loader),
        enter_runtime,
        module_path,
    )
}

/// Execute one linked module with the realm-owned Node host explicitly installed.
///
/// This is the module-graph counterpart of [`execute_node`]. Keeping it separate from
/// [`execute_module_in_runtime`] makes deterministic graph execution the inert default while a
/// host-backed caller opts into ambient I/O at one visible boundary.
pub fn execute_node_module_in_runtime<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
) -> Result<ModuleExecutionResult, ExecutionError> {
    let mut session = ModuleRealmSession::default();
    execute_node_module_in_session(
        module,
        import_values,
        host,
        limits,
        runtime,
        &mut session,
        capabilities,
        true,
    )
}

/// Execute one Node module while preserving realm resources and jobs for subsequent modules.
#[allow(clippy::too_many_arguments)]
pub fn execute_node_module_in_session<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    session: &mut ModuleRealmSession,
    capabilities: ExecutionCapabilities,
    drain: bool,
) -> Result<ModuleExecutionResult, ExecutionError> {
    execute_module_in_runtime_mode_with_session(
        module,
        import_values,
        &[],
        host,
        limits,
        runtime,
        capabilities,
        Some((session, drain)),
        None,
        true,
        None,
    )
}

/// Execute one Node module with a graph-owned lazy CommonJS loader installed.
#[allow(clippy::too_many_arguments)]
pub fn execute_node_module_in_session_with_loader<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    self_imports: &[ModuleSelfImport],
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    session: &mut ModuleRealmSession,
    capabilities: ExecutionCapabilities,
    drain: bool,
    loader: &mut dyn CommonJsModuleLoader,
    enter_runtime: bool,
    module_path: Option<&str>,
) -> Result<ModuleExecutionResult, ExecutionError> {
    execute_module_in_runtime_mode_with_session(
        module,
        import_values,
        self_imports,
        host,
        limits,
        runtime,
        capabilities,
        Some((session, drain)),
        Some(loader),
        enter_runtime,
        module_path,
    )
}

pub(super) fn execute_module_in_runtime_mode<H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    host: &mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
    node: bool,
) -> Result<ModuleExecutionResult, ExecutionError> {
    if node {
        let mut session = ModuleRealmSession::default();
        execute_module_in_runtime_mode_with_session(
            module,
            import_values,
            &[],
            host,
            limits,
            runtime,
            capabilities,
            Some((&mut session, true)),
            None,
            true,
            None,
        )
    } else {
        execute_module_in_runtime_mode_with_session(
            module,
            import_values,
            &[],
            host,
            limits,
            runtime,
            capabilities,
            None,
            None,
            true,
            None,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_module_in_runtime_mode_with_session<'host, H: HostOps + ?Sized>(
    module: &ModuleProgram,
    import_values: &[Value],
    self_imports: &[ModuleSelfImport],
    host: &'host mut H,
    limits: ExecutionLimits,
    runtime: &mut RealmRuntime,
    capabilities: ExecutionCapabilities,
    session: Option<(&mut ModuleRealmSession, bool)>,
    mut commonjs_loader: Option<&'host mut dyn CommonJsModuleLoader>,
    enter_runtime: bool,
    module_path: Option<&str>,
) -> Result<ModuleExecutionResult, ExecutionError> {
    if import_values.len() != module.imports.len() {
        return Err(ExecutionError::ImportCountMismatch {
            expected: module.imports.len(),
            actual: import_values.len(),
        });
    }

    initialize_host_clock(runtime, host)?;

    let commonjs_module = import_values.get(2).cloned();
    let mut locals = vec![Value::Undefined; module.program.local_count as usize];
    for (binding, value) in module.imports.iter().zip(import_values) {
        let Some(local) = locals.get_mut(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        // An import slot always holds a cell, because an import is a live view of the
        // exporter's binding. When the linker hands over the exporter's own cell that view is
        // genuinely live; when it hands over a plain value -- a re-export, a star-expanded name,
        // a namespace object -- it is wrapped here and reads as a snapshot, which is what those
        // forms are until the linker learns to thread cells through them too.
        *local = match value {
            Value::Cell(_) => value.clone(),
            other => Value::Cell(Rc::new(RefCell::new(other.clone()))),
        };
    }
    // A slot the body reads as a cell (a capture, `LoadCell`, `StoreCell`) that no `NewCell`
    // creates is an import's -- and the linker drops an import nothing live reads. A function
    // that only a dead declaration reaches still captures the slot when the body hoists it, so
    // the slot needs a cell even though no value will ever arrive in it. The native backend's
    // locals start `undefined` and read a dropped import the same way (realms issue 22).
    for slot in module_cell_slots_without_new_cell(&module.program) {
        if let Some(local @ Value::Undefined) = locals.get_mut(slot as usize) {
            *local = Value::Cell(Rc::new(RefCell::new(Value::Undefined)));
        }
    }

    // An import this module makes from itself names a binding that does not exist yet, so
    // there was no value for the loop above to place. Both kinds are set up here instead: a
    // named one gets a cell the body's own `NewCell` will replace with the export's, and a
    // namespace gets its object now -- with every name already in it, in the order a namespace
    // holds them -- because the body may read it before it has finished filling it in.
    let mut self_bindings = Vec::new();
    let mut self_namespaces = Vec::new();
    // Every slot a self-import shows starts out holding the dead-zone marker. An exported
    // binding is boxed only when it can change, so a `const`, a class or an `export default`
    // expression is a plain slot that would otherwise read as `undefined` for as long as its
    // declaration had not run -- and `undefined` is the wrong answer to a dead-zone read. The
    // marker is invisible by any other route: an unboxed binding cannot be captured, and a
    // direct read of one in its dead zone is a compile error before it ever runs.
    for self_import in self_imports {
        let slots: &[u32] = match self_import {
            ModuleSelfImport::Binding { export_slot, .. } => std::slice::from_ref(export_slot),
            ModuleSelfImport::Namespace { .. } => &[],
        };
        let namespace_slots: Vec<u32> = match self_import {
            ModuleSelfImport::Namespace { exports, .. } => exports
                .iter()
                .filter_map(|(_, source)| match source {
                    ModuleSelfExport::Slot(slot) => Some(*slot),
                    ModuleSelfExport::Settled(_) => None,
                })
                .collect(),
            ModuleSelfImport::Binding { .. } => Vec::new(),
        };
        for slot in slots.iter().chain(namespace_slots.iter()) {
            let Some(local) = locals.get_mut(*slot as usize) else {
                return Err(ExecutionError::LocalOutOfBounds {
                    index: *slot as usize,
                });
            };
            *local = Value::Native(Native::Uninitialized, RealmId::PRIMARY);
        }
    }
    for self_import in self_imports {
        let (import_slot, seed) = match self_import {
            ModuleSelfImport::Binding {
                import_slot,
                export_slot,
            } => {
                let cell = Rc::new(RefCell::new(Value::Native(
                    Native::Uninitialized,
                    RealmId::PRIMARY,
                )));
                self_bindings.push((Rc::clone(&cell), *export_slot));
                let Some(local) = locals.get_mut(*import_slot as usize) else {
                    return Err(ExecutionError::LocalOutOfBounds {
                        index: *import_slot as usize,
                    });
                };
                *local = Value::Cell(cell);
                continue;
            }
            ModuleSelfImport::Namespace {
                import_slot,
                exports,
            } => {
                let object = namespace_object(exports.iter().map(|(name, source)| {
                    let value = match source {
                        ModuleSelfExport::Slot(_) => {
                            Value::Native(Native::Uninitialized, RealmId::PRIMARY)
                        }
                        ModuleSelfExport::Settled(value) => value.clone(),
                    };
                    (name.clone(), value)
                }));
                if let Value::Object(data) = &object {
                    self_namespaces.push((Rc::clone(data), exports.clone()));
                }
                (*import_slot, object)
            }
        };
        let Some(local) = locals.get_mut(import_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: import_slot as usize,
            });
        };
        *local = Value::Cell(Rc::new(RefCell::new(seed)));
    }

    // Cloned into an `Rc` because a function value this module exports outlives the run that
    // made it: the importer calls it against *this* table, and holding it by reference would tie
    // every exported function to one module execution.
    // The table comes from the compiler rather than being assumed: an ESM module is strict
    // throughout, but a CommonJS one is a sloppy function body in node, and assuming `always`
    // here is what made `this` in a bare call inside a bundle `undefined`.
    if session.is_some() {
        if let Some(fs) = host.clone_fs() {
            runtime.set_fs(fs);
        }
    }
    let program = Rc::new(module.program.clone());
    let strictness = Rc::new(module.strictness.clone());
    let mut vm = Vm::with_entropy_seed(
        program,
        strictness,
        host,
        locals,
        limits,
        runtime.capabilities(),
        capabilities,
    )?;
    vm.host_clock_origin_ms = (runtime.capabilities().clock == ClockCapability::Host)
        .then(|| runtime.host_clock_anchor());
    vm.self_bindings = self_bindings;
    vm.module_path = module_path.map(Rc::from);
    vm.self_namespaces = self_namespaces;
    if session.as_ref().is_some_and(|(session, _)| session.adapter) {
        vm.async_host = Some(runtime as *mut RealmRuntime as *mut dyn ActiveAsyncHostOps);
    }
    // The HTTP runtime is available for global fetch (a WHATWG web standard, not a Node API).
    // The realm's HTTP transport (Hyper via Tokio) exists in both modes; only the Node.js
    // API surface (require, process, etc.) remains gated behind the adapter.
    // The http_client_host allows the HTTP client to establish TCP connections without
    // the full async_host (which would also enable node:fs, node:net, etc.).
    vm.http_runtime = Some(runtime as *mut RealmRuntime as *mut dyn ActiveHttpRuntime);
    vm.http_client_host = Some(runtime as *mut RealmRuntime as *mut dyn ActiveAsyncHostOps);
    if let Some(loader) = commonjs_loader.as_deref_mut() {
        vm.commonjs_loader = Some(loader as *mut dyn CommonJsModuleLoader);
    }
    let drain = session.as_ref().is_some_and(|(_, drain)| *drain);
    if let Some((session, _)) = session.as_ref() {
        vm.module_realm = Some(Rc::clone(&session.shared));
        vm.readonly_cells = Rc::clone(&session.shared.borrow().readonly_cells);
        vm.intrinsics = Rc::clone(&session.shared.borrow().intrinsics);
        vm.isolated_graphs = Rc::clone(&session.shared.borrow().isolated_graphs);
        vm.actor_realm_id = session.shared.borrow().actor_realm_id;
        vm.actor_graph_requests = Rc::clone(&session.shared.borrow().actor_graph_requests);
        vm.global_objects = session.shared.borrow().global_objects.clone();
        vm.module_map = Rc::clone(&session.shared.borrow().module_map);
    }
    // The realm is built and bootstrapped before the body's first instruction: by this machine
    // if it is the first of its session to run, and found already built otherwise.
    let realm = vm.current_realm;
    let execution = match if enter_runtime {
        runtime.enter(|| {
            vm.realize_realm(realm)?;
            vm.run_module_body()
        })
    } else {
        vm.realize_realm(realm).and_then(|_| vm.run_module_body())
    } {
        Ok(execution) => execution,
        Err(error) => {
            if let Some(value) = vm.uncaught_value.take()
                && let Some(loader) = commonjs_loader.as_deref_mut()
            {
                loader.record_thrown_value(value);
            }
            return Err(error);
        }
    };
    // A dependency's top-level await must finish before its importer reads the exported cells.
    // Node graph execution normally leaves a dependency's shared-realm jobs queued so they run
    // after the entry body, but that is only valid for ordinary module jobs. A suspended module
    // owns the VM, program and locals needed by `Sink::ModuleAwait`; keep that VM alive and drain
    // it now, rather than letting the entry VM try to resume a continuation whose owner is gone.
    let suspended_module = vm.module_suspension.is_some();
    let static_module_completion = if suspended_module && vm.commonjs_loader.is_some() {
        vm.module_path.as_deref().and_then(|path| {
            vm.module_map.borrow().completion_in_generation(
                vm.current_realm,
                vm.module_generation,
                path,
            )
        })
    } else {
        None
    };
    vm.esm_body_pending_checkpoint = commonjs_module.is_none();
    // A body that ran to its end has been evaluated: its bindings are final, and the realm's
    // module map learns them now, before the event loop runs the jobs the body queued -- one of
    // which may be an `import()` of this very module. A module suspended at a top-level `await`
    // is reported when it settles, below.
    if !suspended_module
        && let (Some(loader), Some(path)) = (commonjs_loader.as_deref_mut(), module_path)
    {
        loader.module_evaluated(path, &own_exports(module, &vm.locals)?);
    }
    // Exports are read before the event loop runs: they are the module's interface, and a timer
    // that mutates a binding afterwards has not changed what the module exported.
    if enter_runtime {
        // An exception that ends the loop is the program's last word; the loader keeps the value
        // as it does for a module body's (a worker reports it to its parent).
        let mut record = |vm: &mut Vm<'_, H>, error: ExecutionError| {
            if let Some(value) = vm.uncaught_value.take()
                && let Some(loader) = commonjs_loader.as_deref_mut()
            {
                loader.record_thrown_value(value);
            }
            error
        };
        if suspended_module {
            vm.run_node_event_loop_until_module(runtime)
                .map_err(|error| record(&mut vm, error))?;
            // The entry is the module the loop is for: once its await has settled, whatever the
            // continuation armed -- a timer, a server -- still has to run. Stopping here left
            // `await x; setTimeout(f)` with `f` never called (fixed 2026-09-19); a dependency
            // returns to its importer instead, which is what `until_module` is for. But an
            // await that settled by *failing* is reported before that drain, not after: a
            // faulted continuation cannot stop what it armed, so waiting for full quiet first
            // holds the loop open exactly as long as the worker it can no longer reach (an
            // actor whose owner died with the error). Node reports the uncaught error and
            // exits, terminating workers with the process; the rejection check below is what
            // reports it here.
            if drain
                && vm
                    .module_outcome()
                    .is_none_or(|outcome| !matches!(outcome, Settled::Rejected(_)))
            {
                vm.run_node_event_loop(runtime)
                    .map_err(|error| record(&mut vm, error))?;
            }
        } else if drain {
            vm.run_node_event_loop(runtime)
                .map_err(|error| record(&mut vm, error))?;
        } else if vm.async_host.is_none() && session.is_none() {
            // A module with no shared realm drains its own loop; one in a shared realm leaves
            // its timers for the entry's loop, as node does.
            vm.run_event_loop(runtime)?;
        }
    }
    // A module that awaited finished somewhere inside that drain rather than at the `run` above,
    // so what it came to is read here. A rejection is reported the way an uncaught throw from a
    // module body without any `await` in it already is: the module failed, and saying so as a
    // rejection nobody handled would describe the same failure less plainly.
    if let Some(Settled::Rejected(reason)) = vm.module_outcome() {
        if static_module_completion.is_some()
            && let Some(path) = vm.module_path.as_deref()
        {
            vm.module_map.borrow_mut().fail_in_generation(
                vm.current_realm,
                vm.module_generation,
                path,
                reason.clone(),
            );
            vm.settle(
                static_module_completion
                    .as_ref()
                    .expect("checked completion"),
                &Settled::Rejected(reason.clone()),
            );
        }
        let trace = captured_trace(&reason);
        let instruction = vm.instruction;
        let message = vm.describe_uncaught(&reason, instruction);
        if let Some(loader) = commonjs_loader.as_deref_mut() {
            loader.record_thrown_value(reason.clone());
        }
        return Err(ExecutionError::Uncaught {
            instruction,
            message,
            error_constructor: vm.error_constructor(&reason),
            trace,
        });
    }
    // The entry awaited something that never settled and the loop has nothing left that could
    // settle it: a forgotten `resolve`, a server closed before the await. Node warns and exits
    // 13; exiting 0 here read as success to every shell and CI until 2026-09-19 (issue 09 of
    // `docs/issues/full-runtime`). A dependency in that state is its importer's to report.
    if suspended_module
        && enter_runtime
        && (drain || vm.async_host.is_none())
        && vm.module_outcome().is_none()
    {
        let _ = host.call(
            HostOp::WriteStderr,
            &[HostValue::from(Value::String(
                "Warning: Detected unsettled top-level await"
                    .to_owned()
                    .into(),
            ))],
        );
        return Err(ExecutionError::ProcessExited(13));
    }
    // The loop refreshes before each instruction, so the body's *last* store is not in the
    // namespace yet. Nothing in this module can see the difference -- it has stopped running --
    // but whoever holds the object afterwards can.
    if !vm.self_bindings.is_empty() || !vm.self_namespaces.is_empty() {
        vm.refresh_self_imports();
    }
    let execution = ExecutionResult {
        steps: vm.steps,
        process_exit_code: vm.process_exit_code(None),
        ..execution
    };
    let exports = own_exports(module, &vm.locals)?;
    if suspended_module
        && let (Some(loader), Some(path)) = (commonjs_loader.as_deref_mut(), module_path)
    {
        loader.module_evaluated(path, &exports);
    }
    if let Some(completion) = static_module_completion
        && let Some(path) = vm.module_path.as_deref()
        && matches!(
            vm.module_map.borrow().status_in_generation(
                vm.current_realm,
                vm.module_generation,
                path,
            ),
            Some(ModuleStatus::Evaluated)
        )
    {
        let namespace = vm.module_map.borrow_mut().namespace_in_generation(
            vm.current_realm,
            vm.module_generation,
            path,
        );
        vm.resolve_promise(&completion, namespace);
    }
    let commonjs_exports = commonjs_module
        .as_ref()
        .map(|module_object| vm.read_property(module_object, "exports", vm.instruction))
        .transpose()?;
    if let Some((session, _)) = session {
        session.shared.borrow_mut().global_objects = vm.global_objects.clone();
    }
    Ok(ModuleExecutionResult {
        execution,
        exports,
        commonjs_exports,
    })
}

/// The values a module's own export slots hold.
fn own_exports(
    module: &ModuleProgram,
    locals: &[Value],
) -> Result<Vec<ModuleExportValue>, ExecutionError> {
    let mut exports = Vec::with_capacity(module.exports.len());
    for binding in &module.exports {
        let Some(value) = locals.get(binding.local_slot as usize) else {
            return Err(ExecutionError::LocalOutOfBounds {
                index: binding.local_slot as usize,
            });
        };
        // The cell itself, not what is in it: the importer holds this module's binding rather
        // than a copy of the value it had when the module finished. `namespace_object` and the
        // star expansion read through it, because those *are* snapshots.
        exports.push(ModuleExportValue {
            name: binding.exported.clone(),
            value: value.clone(),
        });
    }
    Ok(exports)
}

/// Slots a module body reads as cells -- captured by a function it makes, or loaded and stored
/// as a cell -- that no `NewCell` in the body creates: the ones whose cell must already be there
/// when the body starts. The native backend's `slots_read_as_cells` is the same walk.
fn module_cell_slots_without_new_cell(program: &Program) -> Vec<u32> {
    let mut needed = HashSet::new();
    let mut created = HashSet::new();
    for instruction in &program.code {
        match instruction {
            Instruction::LoadCell(slot) | Instruction::StoreCell(slot) => {
                needed.insert(*slot);
            }
            Instruction::NewCell(slot) => {
                created.insert(*slot);
            }
            Instruction::MakeFunction(proto) => {
                if let Some(target) = program.functions.get(*proto as usize) {
                    for upvalue in &target.upvalues {
                        if let Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } =
                            upvalue
                        {
                            needed.insert(*slot);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    needed.retain(|slot| !created.contains(slot));
    let mut slots: Vec<u32> = needed.into_iter().collect();
    slots.sort_unstable();
    slots
}

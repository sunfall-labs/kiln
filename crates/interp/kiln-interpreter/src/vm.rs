use super::*;
#[path = "vm/array_like.rs"]
mod array_like;
#[path = "vm/arrays.rs"]
mod arrays;
#[path = "vm/calls.rs"]
mod calls;
#[path = "vm/execution.rs"]
mod execution;
#[path = "vm/host.rs"]
mod host;
#[path = "vm/intrinsics.rs"]
mod intrinsics;
#[path = "vm/iterators.rs"]
mod iterators;
#[path = "vm/modules.rs"]
mod modules;
#[path = "vm/object.rs"]
mod object;
#[path = "vm/promises.rs"]
mod promises;
#[path = "vm/prototypes.rs"]
mod prototypes;
#[path = "vm/realm.rs"]
mod realm;
#[path = "vm/workers.rs"]
mod workers;

/// Instructions a worker's machine runs between checks of its terminate flag: few enough that
/// `worker.terminate()` stops a busy loop within a fraction of a millisecond, many enough that the
/// check costs nothing measurable.
pub(crate) const WORKER_QUANTUM: u64 = 100_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadonlyCellKind {
    Const,
    FunctionName,
}

#[derive(Default)]
pub(crate) struct ReadonlyCellRegistry {
    entries: std::collections::HashMap<usize, ReadonlyCellEntry>,
    sweep: std::collections::VecDeque<usize>,
    queued: std::collections::HashSet<usize>,
}

struct ReadonlyCellEntry {
    cell: Weak<RefCell<Value>>,
    kind: ReadonlyCellKind,
    /// A later lexical declaration in the caller may still need its one initialization store.
    /// Only that exact program may consume the exception; eval writes and escaped closures may
    /// not initialize the binding early.
    initializer_program: Option<Weak<Program>>,
}

pub(crate) type SharedReadonlyCellRegistry = Rc<RefCell<ReadonlyCellRegistry>>;

impl ReadonlyCellRegistry {
    pub(crate) fn mark(
        &mut self,
        cell: &Cell,
        kind: ReadonlyCellKind,
        initializer_program: Option<&Rc<Program>>,
    ) {
        let key = Rc::as_ptr(cell) as usize;
        if let Some(entry) = self.entries.get_mut(&key) {
            if entry
                .cell
                .upgrade()
                .is_some_and(|known| Rc::ptr_eq(&known, cell))
            {
                entry.kind = kind;
                if entry.initializer_program.is_none()
                    && matches!(&*cell.borrow(), Value::Native(Native::Uninitialized, _))
                {
                    entry.initializer_program = initializer_program.map(Rc::downgrade);
                }
                return;
            }
        }
        let initializer_program =
            matches!(&*cell.borrow(), Value::Native(Native::Uninitialized, _))
                .then(|| initializer_program.map(Rc::downgrade))
                .flatten();
        self.entries.insert(
            key,
            ReadonlyCellEntry {
                cell: Rc::downgrade(cell),
                kind,
                initializer_program,
            },
        );
        if self.queued.insert(key) {
            self.sweep.push_back(key);
        }
        self.sweep_dead(16);
    }

    pub(crate) fn kind_for_write(
        &mut self,
        cell: &Cell,
        program: &Rc<Program>,
        initializing_store: bool,
    ) -> Option<ReadonlyCellKind> {
        let key = Rc::as_ptr(cell) as usize;
        match self.entries.get_mut(&key) {
            Some(entry) => match entry.cell.upgrade() {
                Some(known) if Rc::ptr_eq(&known, cell) => {
                    let is_uninitialized =
                        matches!(&*cell.borrow(), Value::Native(Native::Uninitialized, _));
                    let is_owner_initializer = entry
                        .initializer_program
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .is_some_and(|owner| Rc::ptr_eq(&owner, program));
                    if initializing_store && is_uninitialized && is_owner_initializer {
                        entry.initializer_program = None;
                        None
                    } else {
                        Some(entry.kind)
                    }
                }
                Some(_) | None => {
                    self.entries.remove(&key);
                    None
                }
            },
            None => None,
        }
    }

    fn sweep_dead(&mut self, budget: usize) {
        for _ in 0..budget {
            let Some(key) = self.sweep.pop_front() else {
                break;
            };
            self.queued.remove(&key);
            let Some(entry) = self.entries.get(&key) else {
                continue;
            };
            if entry.cell.strong_count() == 0 {
                self.entries.remove(&key);
            } else if self.queued.insert(key) {
                self.sweep.push_back(key);
            }
        }
    }
}

pub(super) struct Vm<'host, H: HostOps + ?Sized> {
    /// The program the *running* frame belongs to, swapped on every call and restored on return.
    pub(super) program: Rc<Program>,
    /// Which of `program`'s bodies the compiler compiled as strict code. Swapped with `program`,
    /// because a function that crossed a module boundary is indexed against its own program.
    pub(super) strictness: Rc<Strictness>,
    /// Whether the *running* frame is strict code.
    ///
    /// The one thing the machine cannot work out for itself at the point of a write: a refused
    /// assignment is silently discarded in sloppy mode and a `TypeError` in strict mode, and the
    /// two are the same instruction. Saved and restored with the frame.
    pub(super) strict: bool,
    pub(super) host: &'host mut H,
    pub(super) stack: Vec<Value>,
    pub(super) locals: Vec<Value>,
    /// The closure currently executing: `None` is the module body.
    pub(super) function: Option<Rc<Closure>>,
    /// Property key used to enter the current function, when it was invoked as a method.
    pub(super) method_name: Option<StringUnits>,
    /// Set by `Yield`, so the run loop knows it stopped at a suspension rather than a return.
    pub(super) suspended: bool,
    /// Active exception handlers, innermost last.
    pub(super) handlers: Vec<Handler>,
    /// The arguments the running frame received.
    pub(super) arguments: Vec<Value>,
    /// The `arguments` object the running frame has handed out, if it has asked for one.
    ///
    /// One object per call, built on first mention. `arguments` is a single object in the
    /// language, and every read of the name produces it again: `arguments[0] = 1` is visible to
    /// the next read, and `arguments === arguments` is true. A fresh array per read made both
    /// false -- a write went into an array nothing else could reach, and every definition made
    /// on it was discarded by the statement after it.
    pub(super) arguments_object: Option<Value>,
    /// Whether the running frame's `arguments` is the *mapped* kind: sloppy code with a simple
    /// parameter list.
    ///
    /// Carried on the frame rather than read at the mention, because an arrow captures the
    /// enclosing call's object where the arrow is *created*, which can be before anything has
    /// mentioned `arguments` at all. Decided by the compiler, which is the only place the
    /// parameter list and the strictness are both in view.
    pub(super) mapped_arguments: bool,
    /// Which realm the running code belongs to, saved and restored with the frame.
    ///
    /// Not a property of the *value* being run but of the execution: an intrinsic minted while
    /// realm 1 is current belongs to realm 1, which is what `GetFunctionRealm` and every
    /// "throw *this* realm's TypeError" step in the specification are asking about.
    pub(super) current_realm: RealmId,
    /// Weak facade identities for the shared graph nodes this execution has read. Each facade
    /// itself retains its graph; this cache never keeps an unreachable graph alive.
    pub(super) shared_facades: Rc<RefCell<SharedCache>>,
    /// Private actor graph roots. Weak handle identities stay in the engine; JavaScript holds
    /// only opaque handles and transfer tickets, never the graph in a module WeakMap.
    pub(super) isolated_graphs: Rc<RefCell<Vec<(Weak<RefCell<ObjectData>>, Value)>>>,
    /// Values reached by WeakRef construction/deref stay alive through the current job only.
    pub(super) weak_ref_keepalive: Vec<Value>,
    pub(super) actor_realm_id: u64,
    pub(super) actor_graph_requests: Rc<RefCell<std::collections::HashMap<u64, u8>>>,
    /// `this` in the running frame. At module scope it is `undefined`, because a module body is
    /// always strict and strict code does not get the global object.
    pub(super) this_value: Value,
    /// The realm's global object, built on first use and shared from then on.
    ///
    /// One object, because `globalThis === globalThis` has to hold and because a library that
    /// stashes state on it -- `globalThis.__something ??= {}` is a common singleton idiom -- has
    /// to find it again. What it does *not* do is back variable lookup: writing
    /// `globalThis.x = 1` does not make a bare `x` resolve, since bindings live in module slots.
    pub(super) global_objects: Vec<Option<Value>>,
    /// The `Timeout` and `Immediate` prototypes a realm's bootstrap published, by realm: what
    /// `timer_handle` gives each handle it allocates, found once rather than by a scan of the
    /// global object's properties on every timer.
    pub(super) timer_prototypes: Vec<Option<(Value, Value)>>,
    /// Successful RegExp capture strings, indexed by realm. Failed matches leave this snapshot
    /// alone, as Annex B requires.
    pub(super) regexp_legacy_captures: Vec<[StringUnits; 9]>,
    /// How many built-ins are currently between the running code and the interpreter loop.
    pub(super) native_depth: usize,
    /// `process`, built on first use and shared from then on.
    ///
    /// Bundles read `process.env.NODE_ENV` unguarded -- it is the standard way a library ships
    /// development-only checks -- and ADR 0017 made an unresolved identifier a runtime throw, so
    /// without this a great deal of ordinary code fails at its first line. What is here is only
    /// what such code reads: an environment that is empty (so every flag reads as undefined, the
    /// production answer), and enough identity to log. It is deliberately not node's `process`
    /// and has no way to exit, spawn, or reach a file system.
    pub(super) process_object: Option<Value>,
    /// The unterminated tail of what `process.stdout.write` / `process.stderr.write` wrote.
    /// Host output operations write whole lines, so a partial line waits here: the next write
    /// completes it, the next `console.log` on the stream continues it, and the end of the run
    /// flushes it (see `Vm::write_stdio`).
    pub(super) pending_stdio: [String; 2],
    /// The built-ins a program has mutated, and how.
    ///
    /// A realm root like `global_object` and `error_prototypes`: `Math.max.name` and
    /// `Array.prototype.push` are answered from tables that no program can reach, and this is
    /// where a program's changes to them live for as long as the realm does.
    pub(super) intrinsics: Rc<Intrinsics>,
    /// Whether the suspension that just happened was an `await` rather than a `yield`.
    ///
    /// Written where the body stops and read by whoever resumed it, because the instruction that
    /// stopped is not otherwise visible from there.
    pub(super) suspended_awaiting: bool,
    pub(super) suspended_delegate: Option<(Value, usize, bool)>,
    /// `new.target` in the running frame: the constructor a `new` call named, or `undefined`.
    ///
    /// Held beside `this` and saved with it, because it follows exactly the same rule -- an
    /// ordinary call has none, and an arrow uses the one from where it was written.
    pub(super) new_target: Value,
    pub(super) frames: Vec<Frame>,
    /// The frame whose locals are module scope for the running code, or the running frame itself
    /// when the stack has not grown past it.
    ///
    /// Zero for every program that contains no `eval`: module scope is the bottom of the stack,
    /// which is what `LoadGlobal` has always meant. An eval program is a second bottom -- it has
    /// its own top-level slots -- and this is what lets both be true at once.
    pub(super) global_base: usize,
    pub(super) instruction: usize,
    /// The explicit language queues and virtual clock owned by the shared Realm Runtime seam.
    pub(super) realm: Realm<Scheduled, Timer>,
    /// Policy for civil time and for advancing timer deadlines during event-loop turns.
    pub(super) clock_capability: ClockCapability,
    /// Shared realm origin used to refresh timer deadlines while synchronous code is running.
    pub(super) host_clock_origin_ms: Option<Rc<std::cell::Cell<Option<f64>>>>,
    /// Canonical environmental strings resolved once when the realm is entered.
    ///
    /// Host-backed policy crosses HostOps here, never from an individual builtin. This makes the
    /// value realm state, gives every consumer one answer, and keeps host reads visible at the
    /// versioned boundary.
    pub(super) timezone: String,
    pub(super) default_locale: String,
    /// Runtime source compilation is explicit realm authority, absent unless the caller selects
    /// a compiler. It is copied into each short-lived module VM from the graph session policy.
    pub(super) source_compiler: Option<SourceCompiler>,
    /// The shared TypeScript built-ins every realm runs once its global object exists
    /// ([`RealmBootstrap`]). Run by [`Vm::realize_realm`], never by the lazy global builder.
    pub(super) realm_bootstrap: Option<RealmBootstrap>,
    /// Whether this execution's host allows a second realm, and therefore publishes the hook
    /// that makes one. False for every ordinary run.
    pub(super) host_realms: bool,
    /// The text a new agent's realm is given before the program's own source, when this
    /// execution's host granted agents at all. `None` disables `$262AgentStart` and its siblings.
    pub(super) host_agents: Option<&'static str>,
    pub(super) module_loader: Option<Arc<dyn ModuleLoaderService + Send + Sync>>,
    /// How this execution starts a worker, if it can (`node:worker_threads`).
    pub(super) worker_host: Option<Arc<dyn crate::WorkerHost>>,
    /// Whether this machine runs in a worker, whose parent may stop it at any checkpoint.
    pub(super) in_worker: bool,
    pub(super) module_path: Option<Rc<str>>,
    /// The command line `process.argv` reports, as the host stated it. Empty when no host did.
    pub(super) argv: Vec<String>,
    /// Promise state waiting for a Tokio-owned host request, keyed by opaque request identity.
    pub(super) pending_host: Vec<PendingHostRequest>,
    /// Persistent Node resource requests routed to provider-neutral server/stream drivers.
    pub(super) pending_node: Vec<PendingNodeRequest>,
    /// Live HTTP servers in this realm. A separate registry makes response-to-stream lookup
    /// independent of which host operations happen to be pending at that moment.
    pub(super) node_servers: Vec<Rc<RefCell<NodeServerState>>>,
    /// Strong roots for server EventEmitter objects from listen until terminal close. This owner
    /// registry avoids a cycle through `ObjectData::node_object` and is released with the VM or
    /// its shared module realm on abort/teardown.
    pub(super) active_node_server_objects: std::collections::HashMap<usize, Value>,
    /// Interpreter-owned ClientRequest values keyed exactly like the runtime Agent queue.
    pub(super) http_client_requests:
        std::collections::BTreeMap<u64, Rc<RefCell<HttpClientRequestState>>>,
    /// Monotonic request identity for standalone execution. Module graphs keep the same counter
    /// in `ModuleRealmShared`, so every request in one realm has a distinct routing key.
    pub(super) next_http_client_request_id: u64,
    /// Stable graph-realm ownership for resources referenced by callbacks created in another VM.
    pub(super) module_realm: Option<Rc<RefCell<ModuleRealmShared>>>,
    /// Runtime-owned live I/O installed only for explicitly Node-compatible execution.
    pub(super) async_host: Option<*mut dyn ActiveAsyncHostOps>,
    /// Runtime-owned HTTP policy installed beside the capability-only HostOps surface.
    pub(super) http_runtime: Option<*mut dyn ActiveHttpRuntime>,
    /// Async host for the HTTP client (global fetch), available without the Node Adapter.
    /// Set alongside http_runtime; the underlying RealmRuntime implements ActiveAsyncHostOps.
    pub(super) http_client_host: Option<*mut dyn ActiveAsyncHostOps>,
    /// Graph-owned lazy CommonJS loader, installed only while evaluating a module graph.
    pub(super) commonjs_loader: Option<*mut (dyn CommonJsModuleLoader + 'host)>,
    /// Which `AsyncLocalStorage`s hold a store right here.
    ///
    /// Realm state that follows *control*, not the frame: it is captured whenever work is queued
    /// and restored whenever that work runs, which is what carries a store across an `await`.
    pub(super) async_context: AsyncContext,
    /// The body of an ES module just ran. Node evaluates one from inside a promise job (the
    /// loader awaits `module.evaluate()`), so the microtask checkpoint that follows the body
    /// is already in progress: promise jobs queued by the body run before any `nextTick` it
    /// queued. A CommonJS body runs from the top of a macrotask and its ticks go first.
    pub(super) esm_body_pending_checkpoint: bool,
    /// The value delivered to a host handler, waiting to be collected by `call_catching`.
    pub(crate) caught: Option<Value>,
    /// Virtual file descriptor table for Node filesystem operations.
    pub(crate) fd_table: fs::VirtualFdTable,
    /// A top-level throw captured for the graph loader before it is converted to an execution
    /// report. This remains inside the realm and never crosses the worker error boundary.
    pub(super) uncaught_value: Option<Value>,
    /// The rejections this machine's checkpoint reports when it has no module session.
    pub(super) rejections: Rejections,
    pub(super) module_suspension: Option<Rc<RefCell<ModuleSuspension>>>,
    pub(super) module_completion: Option<Rc<PromiseData>>,
    /// The cell a module's import of one of its own exports reads, beside the slot it shows.
    ///
    /// Not the export's own cell: an exported binding is boxed only when it can change, so half
    /// of them are plain slots with no cell to share. Caught up from the slot instead, which is
    /// one mechanism for both rather than a fast path and a fallback.
    /// Empty for every module that does not import from itself, which is nearly all of them.
    pub(super) self_bindings: Vec<(Rc<RefCell<Value>>, u32)>,
    /// Immutable direct-eval bindings keyed by cell identity. Weak keys let closures keep the
    /// cells alive naturally without retaining every evaluated function for the VM's lifetime.
    pub(super) readonly_cells: SharedReadonlyCellRegistry,
    /// Namespace objects a module imported from itself, and the slots each name shows. Caught up
    /// from those slots before every instruction, so no read can see one stale.
    pub(super) self_namespaces: Vec<SelfNamespace>,
    /// `Symbol.for`'s registry: one symbol per key, for the whole run. Its point is that two
    /// lookups of one key are the *same* symbol, which is what makes it usable across module
    /// boundaries where `Symbol()` would produce two.
    pub(super) symbol_registry: Vec<(StringUnits, Value)>,
    /// Canonical tagged-template literal objects cached by (RealmId, program pointer, site index)
    /// per ECMA-262 §13.2.8.5 (GetTemplateObject).
    pub(super) template_objects: TemplateObjectCache,
    /// The state `Math.random` advances. Seeded from a constant rather than from the environment
    /// (ADR 0026), so the same program produces the same sequence in both engines and on every
    /// machine -- the same trade the virtual clock makes.
    pub(super) entropy: EntropyCapability,
    pub(super) random_state: u64,
    pub(super) steps: u64,
    /// The step count at which the dispatch loop stops to think.
    ///
    /// `min(max_steps, this quantum's end)`. For a program with one agent it *is* `max_steps`,
    /// so the loop pays exactly the comparison it always paid; for a program with two, it is the
    /// point the baton moves. Ordering the two boundaries into one number is what keeps the
    /// agent scheduler off the hot path of every program that has no second agent.
    pub(super) checkpoint: u64,
    /// The agents of this program, once its host has granted any.
    ///
    /// Present on the main agent's machine and on every agent's, because `Atomics.wait` is asked
    /// from either side. `None` for every execution whose host did not ask for agents, which is
    /// every execution but a conformance run.
    pub(super) agents: Option<Arc<AgentGroup>>,
    /// This machine's agent, zero for the main one.
    pub(super) agent_id: usize,
    /// `Atomics.waitAsync` promises this agent is holding, by the scheduler's ticket.
    ///
    /// The promise cannot go to the scheduler -- it is `Rc` and belongs to this realm -- so the
    /// scheduler holds the ticket, and this is where the two are matched up again.
    pub(super) async_waits: Vec<(u64, Rc<PromiseData>)>,
    /// Held only by the main agent, and only so that dropping the machine stops the threads.
    pub(super) agent_owner: Option<AgentOwner>,
    /// Serial for the next `Symbol(...)`. Monotonic per execution, which is all the identity a
    /// symbol needs: it never leaves the run that made it.
    pub(super) symbol_serial: u64,
    /// Every `Symbol(...)` this run has made, by serial.
    ///
    /// A symbol-keyed property stores only the *string* the symbol names it under, so nothing on
    /// the object can hand the symbol itself back -- and `Object.getOwnPropertySymbols` has to
    /// hand back the same symbol, by identity, not a fresh one with the same description.
    /// Strongly held, because the last reference to a symbol can legitimately be the property
    /// key it is stored under: `const o = {}; o[Symbol("k")] = 1;` keeps nothing else, and node
    /// still reports it.
    pub(super) symbols: Vec<Rc<UniqueSymbol>>,
    /// Every closure this run has created, held weakly.
    ///
    /// `F.prototype.constructor` is `F`, and `Rc` frees nothing that is inside a cycle, so every
    /// function a program evaluates stays allocated until the process exits. A program that runs
    /// once never notices; a harness that runs the whole of Test262 in one process grows by the
    /// size of every function it has ever seen and is killed well before the end. Weak, so the
    /// registry itself keeps nothing alive -- it exists only so [`Vm::release_cycles`] can find
    /// the closures again and cut the links.
    pub(super) closures: Vec<Weak<Closure>>,
    /// The realm's module map (see [`ModuleMap`]): what each module has come to, by realm and
    /// canonical key. Shared with every other machine in this realm session, and with the static
    /// graph executor, so a module is evaluated once however it is reached. A machine running
    /// outside a session has a map of its own.
    pub(super) module_map: SharedModuleMap,
    pub(super) limits: ExecutionLimits,
}

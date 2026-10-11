use super::*;

/// The language payload attached to a Realm Runtime timer.
#[derive(Clone, Debug)]
pub(super) enum Timer {
    /// A timer that has no handle of its own (`AbortSignal.timeout`, an `Atomics.waitAsync`
    /// deadline, a realm whose bootstrap published no handles): the callback is held here.
    Plain {
        callback: Value,
        arguments: Vec<Value>,
        /// The async context in force when the timer was *set*, restored while its callback
        /// runs.
        ///
        /// Captured at scheduling rather than read at firing, which is the whole of what makes
        /// an `AsyncLocalStorage` work: `als.run(store, () => setTimeout(f))` has long returned
        /// by the time `f` runs, and `f` must still see the store.
        context: AsyncContext,
    },
    /// A `Timeout` or `Immediate`: the handle object is the record (`TimerRecord`), and this is
    /// the queue's one reference to it. An interval's re-arm clones the reference, not the
    /// callback.
    Handle(Rc<RefCell<ObjectData>>),
}

impl Timer {
    pub(super) fn plain(callback: Value, arguments: Vec<Value>, context: AsyncContext) -> Self {
        Self::Plain {
            callback,
            arguments,
            context,
        }
    }

    /// What to call when the timer fires, or nothing when it was cleared. A timeout marks its
    /// record as having run first, so a `refresh()` inside the callback arms it again.
    pub(super) fn into_call(self) -> Option<(Value, Vec<Value>, AsyncContext)> {
        match self {
            Self::Plain {
                callback,
                arguments,
                context,
            } => Some((callback, arguments, context)),
            Self::Handle(handle) => {
                let mut data = handle.borrow_mut();
                let Some(NodeObject::TimerHandle(record)) = &mut data.node_object else {
                    return None;
                };
                if record.cleared {
                    return None;
                }
                if !record.interval {
                    record.ran = true;
                }
                Some((
                    record.callback.clone(),
                    record.arguments.clone(),
                    Rc::clone(&record.context),
                ))
            }
        }
    }
}

/// One `AsyncLocalStorage`, whose identity is the whole of it.
///

/// A promise: a value that is not there yet, and the reactions waiting for it.
///
/// Its own value kind rather than an object with reserved keys, unlike Set and Map, because the
/// waiting reactions are machine state rather than program-visible properties -- putting them in
/// an object would mean a spread or a `JSON.stringify` could reach them.
#[derive(Debug, PartialEq)]
pub struct PromiseState {
    pub(super) settled: Option<Settled>,
    /// Reactions registered while still pending, in the order `then` was called.
    pub(super) reactions: Vec<Reaction>,
    /// Whether a rejection has somewhere to go: the specification's [[PromiseIsHandled]].
    pub(super) handled: bool,
    /// Where a rejection that nothing handled stands in node's tracking.
    pub(super) rejection: RejectionReport,
}

/// Where a promise that rejected with nothing attached stands in node's unhandled-rejection
/// tracking (`kiln-builtins/src/rejections.ts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RejectionReport {
    /// Never rejected unhandled, or handled before its checkpoint ended.
    #[default]
    None,
    /// Queued for the end of this checkpoint, unless something handles it first.
    Waiting,
    /// Handed to the tracking, so a handler attached now is `rejectionHandled`.
    Reported,
}

/// The rejections a checkpoint has to report, beside the tracking that reports them.
///
/// One per realm session, like the job queues: a promise settled by one module's machine is
/// reported by whichever machine ends the checkpoint.
#[derive(Debug, Default)]
pub(super) struct Rejections {
    /// Promises rejected with nothing attached, in order, with their reasons. An entry whose
    /// promise is no longer `Waiting` was handled in time and is skipped.
    pub(super) rejected: Vec<(Rc<PromiseData>, Value)>,
    /// `Reported` promises something has handled since.
    pub(super) handled: Vec<Rc<PromiseData>>,
    /// The shared tracking's entry point, registered by the realm bootstrap. Without one (a realm
    /// with no bootstrap) the first rejection is reported as an uncaught exception.
    pub(super) process: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Settled {
    Fulfilled(Value),
    Rejected(Value),
}

/// One `then` waiting on a promise: what to run, and where the result goes.
#[derive(Debug, PartialEq)]
pub(super) struct Reaction {
    pub(super) on_fulfilled: Value,
    pub(super) on_rejected: Value,
    pub(super) sink: Sink,
    /// `finally`: run the handler, then settle the sink with the *original* outcome rather than
    /// the handler's result. That is the whole difference from `then(f, f)`, which would replace
    /// a rejection with a fulfilment.
    pub(super) passthrough: bool,
    /// The async context in force where this reaction was *attached*.
    ///
    /// Attached, not settled, and the difference is the whole of what makes an
    /// `AsyncLocalStorage` usable. A promise is very often settled by code with a context of its
    /// own -- `Promise.all` resolves inside whichever input finished last -- and a handler that
    /// inherited *that* would see a store belonging to some unrelated task. `p.then(f)` captures
    /// where `then` was written, and an `await` captures where it suspended, which is the same
    /// rule stated twice.
    pub(super) context: AsyncContext,
}

/// Where a settled outcome goes.
#[derive(Debug, PartialEq)]
pub(super) enum Sink {
    /// The promise `then` returned.
    Promise(Rc<PromiseData>),
    /// A dynamic import waiting for one module in its dependency plan. Once that module settles,
    /// the request is retried against the same importer and generation; this advances the plan
    /// without allowing a dependent body to run before its asynchronous dependencies complete.
    ImportRetry(Rc<ImportRetry>),
    /// Wake a module-plan scheduler when any one of its pending dependencies settles. The
    /// scheduler then rechecks all dependency states and starts every newly ready component.
    ModulePlanWake(Rc<PromiseData>),
    /// Settle a promise with a *recorded* outcome once something else has settled, unless that
    /// something rejected. `finally` waits on its handler's result and then restores the outcome
    /// it was passing through.
    Replay(Rc<PromiseData>, Settled),
    /// An `await` inside an async function: resume its body with whatever the awaited promise
    /// produced, and settle the function's own promise when the body finishes.
    Async(Rc<GeneratorData>, Rc<PromiseData>),
    /// An `await` inside an async *generator*: resume its body, and settle the promise the
    /// consumer's `next()` returned once the body reaches its next `yield` or finishes.
    ///
    /// The promise here belongs to one step rather than to the whole function, which is the
    /// whole difference between the two.
    AsyncGenerator(Rc<GeneratorData>, Rc<PromiseData>),
    /// One awaited disposer inside `AsyncDisposableStack.prototype.disposeAsync`. Like the two
    /// above, there is no JavaScript handler: what resumes is the walk down the stack.
    AsyncDisposal(Rc<AsyncDisposal>),
    /// The value an async generator yielded, being awaited before it is handed to the consumer.
    ///
    /// `yield v` in an async generator is `Await(v)` and then the yield, which is why an async
    /// generator that yields promises hands back their *values*. Without this the consumer got
    /// the promise itself, and `for await (const x of gen())` bound `x` to a promise.
    AsyncGeneratorYield(Rc<GeneratorData>, Rc<PromiseData>),
    ModuleAwait(Rc<RefCell<ModuleSuspension>>),
    /// `AsyncGeneratorAwaitReturn`: the argument of a `return()` on a generator with no body
    /// left to unwind, being awaited before it becomes the final step.
    ///
    /// The generator is already finished; this sink only decides the step this `return()`
    /// promised after its value settles.
    AsyncGeneratorReturn(Rc<GeneratorData>, Rc<PromiseData>),
    /// Awaiting the completion value of `return()` on a suspended async generator before
    /// injecting the return completion into its body.
    AsyncGeneratorReturnActive(Rc<GeneratorData>, Rc<PromiseData>),
    /// Awaiting the `return` argument when the delegate iterator has no `return` method.
    AsyncGeneratorDelegateReturnMissing(Rc<GeneratorData>, Rc<PromiseData>),
    /// Awaiting the result of calling `return()` to close the delegate when `throw()` is missing.
    AsyncGeneratorDelegateClose(Rc<GeneratorData>, Rc<PromiseData>),
    /// Awaiting the result of calling `return()` on an active delegate iterator.
    AsyncGeneratorDelegateReturn(Rc<GeneratorData>, Rc<PromiseData>, bool),
    /// Awaiting the result of calling `throw()` on an active delegate iterator.
    AsyncGeneratorDelegateThrow(Rc<GeneratorData>, Rc<PromiseData>, usize, bool),
    /// Awaiting an inner promise-valued `.value` from a wrapped synchronous iterator during delegation.
    AsyncGeneratorDelegateStep {
        generator: Rc<GeneratorData>,
        step: Rc<PromiseData>,
        exit_target: usize,
        done: bool,
        is_return: bool,
    },
}

#[derive(Debug)]
pub(super) struct ImportRetry {
    pub(super) promise: Rc<PromiseData>,
    pub(super) specifier: String,
    pub(super) options: Option<ImportAttributes>,
    pub(super) importer: Option<Rc<str>>,
    pub(super) generation: u64,
    pub(super) generation_owner: crate::module_map::ModuleGenerationHandle,
}

impl PartialEq for ImportRetry {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.promise, &other.promise)
            && self.specifier == other.specifier
            && self.options == other.options
            && self.importer == other.importer
            && self.generation == other.generation
            && Rc::ptr_eq(&self.generation_owner, &other.generation_owner)
    }
}

/// One `AsyncDisposableStack.prototype.disposeAsync` in flight.
///
/// On the heap because the walk is suspended at every disposer: what a synchronous loop would
/// keep on the Rust stack has to survive a return to the microtask queue.
#[derive(Debug, PartialEq)]
pub(super) struct AsyncDisposal {
    /// The promise the caller is holding.
    pub(super) result: Rc<PromiseData>,
    /// The disposers still to run, in the order they were recorded: taken from the back, so
    /// each resource is released before the ones it was built on top of.
    pub(super) pending: RefCell<Vec<Value>>,
    /// What the throws so far add up to -- one of them travelling as itself, several folded
    /// into a chain of `SuppressedError`s.
    pub(super) thrown: RefCell<Option<Value>>,
    /// Whether a resource with *no* disposer was reached. `await using x = null` records one, so
    /// that disposal still costs the tick the declaration would have cost had the value had a
    /// disposer -- otherwise a program's job ordering would depend on whether a resource
    /// happened to be null.
    pub(super) needs_await: std::cell::Cell<bool>,
    /// Whether anything has actually been awaited, which is what makes that make-up tick
    /// unnecessary.
    pub(super) has_awaited: std::cell::Cell<bool>,
}

/// The state one `Promise.all`/`allSettled`/`any` walk shares across its element functions.
///
/// The specification keeps this in the element functions' internal slots, all of them pointing
/// at the same `values` list and the same `remainingElementsCount`. That sharing is the whole
/// mechanism: the count starts at one for the walk itself, rises once per input, and whichever
/// element brings it to zero is the one that resolves the capability -- which is what makes the
/// walk finishing and the last input settling the same event, in either order.
pub(super) struct CombinatorWalk {
    pub(super) native: Native,
    pub(super) values: RefCell<Vec<Value>>,
    pub(super) remaining: std::cell::Cell<usize>,
    pub(super) resolve: Value,
    pub(super) reject: Value,
}

/// One `Promise.all` Resolve Element Function, and its `allSettled`/`any` counterparts.
///
/// A real function object rather than a Rust closure because a program is handed it and may do
/// anything with it: a thenable's `then` receives it, keeps it, calls it twice. `alreadyCalled`
/// is per element and the rest is shared -- see [`CombinatorWalk`].
pub struct ElementFunction {
    pub(super) walk: Rc<CombinatorWalk>,
    pub(super) index: usize,
    /// Whether this is the *reject* half, which only `allSettled` and `any` have.
    pub(super) rejects: bool,
    /// Shared with this input's *other* element function, because the specification gives the
    /// pair one `alreadyCalled` record between them rather than one each. A thenable that calls
    /// both must have only the first take effect, and two separate flags let both through and
    /// settled the same slot twice.
    pub(super) already_called: Rc<std::cell::Cell<bool>>,
}

impl std::fmt::Debug for ElementFunction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ElementFunction")
    }
}

impl PromiseState {
    pub(super) fn pending() -> Rc<PromiseData> {
        Rc::new(PromiseData {
            state: RefCell::new(Self {
                settled: None,
                reactions: Vec::new(),
                handled: false,
                rejection: RejectionReport::None,
            }),
            properties: RefCell::new(ObjectData::default()),
        })
    }
}

/// A promise: the state the specification gives it, and the ordinary properties a program hung
/// beside it.
///
/// The store is here for the same reason a date has one. A promise is exotic in exactly one
/// respect -- what it is waiting for -- and an ordinary object in every other, so `p.tag = 1` is
/// a property write and `class P extends Promise {}` records its prototype the way every
/// subclass does. Without it the first threw and the second produced a promise that was not an
/// instance of its own class.
#[derive(Debug)]
pub struct PromiseData {
    pub(super) state: RefCell<PromiseState>,
    pub(super) properties: RefCell<ObjectData>,
}

impl PartialEq for PromiseData {
    /// Identity, like every other object: two promises are equal only when they are one promise.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// One unit of work on the microtask queue.
///
/// Both kinds drain together and in order, because the language puts them on one queue: a
/// `queueMicrotask` between two `then`s runs between them.
#[derive(Debug)]
pub(super) enum Job {
    /// Node's next-tick queue drains ahead of the ordinary ECMAScript microtask queue. It uses
    /// the same realm owner and checkpoint, but a distinct lane so Promise jobs cannot overtake
    /// callbacks Node promises will run first.
    NextTick(Value, Vec<Value>),
    /// `queueMicrotask(callback)`.
    Callback(Value, Vec<Value>),
    /// A settled promise's reaction, waiting to be run.
    React(Settled, Reaction),
    /// Attach `outer` to `inner`, for a promise resolved with another promise.
    ///
    /// A job rather than an immediate attachment because the specification resolves a thenable by
    /// *queueing* a call to its `then`, which costs a tick -- so returning a promise from a
    /// handler settles one turn later than returning a plain value. That is observable, and
    /// programs order themselves around it.
    Adopt(Rc<PromiseData>, Rc<PromiseData>),
    /// Hand this realm's resolving functions to a foreign thenable's `then`.
    ///
    /// A job for the same reason `Adopt` is one: the specification *queues* the call, so a
    /// thenable settles one tick later than a plain value. Programs order themselves around that
    /// tick, and running the `then` straight away would move them.
    Thenable(Rc<PromiseData>, Value, Value),
    /// `import(specifier)`: resolve, evaluate and settle, with the specifier already coerced.
    ///
    /// A job rather than work the instruction does, because the specification loads a module
    /// asynchronously even when the host already has it in memory. Running it inline would
    /// settle the promise before `import(x).then(f)` had attached `f`, which is not observable
    /// on its own -- but the module's *side effects* would then happen before the statement
    /// after the `import()`, and that is.
    Import(
        Rc<PromiseData>,
        String,
        Option<ImportAttributes>,
        Option<Rc<str>>,
        u64,
        crate::module_map::ModuleGenerationHandle,
    ),
    /// `ShadowRealm.prototype.importValue(specifier, exportName)`.
    ///
    /// A job for the same reason `Import` is one, and carries the two realms because both
    /// matter: the module is loaded and evaluated in `evaluated`, and the export it names is
    /// wrapped into `caller` on the way back.
    ShadowImport {
        promise: Rc<PromiseData>,
        specifier: String,
        export: String,
        evaluated: RealmId,
        caller: RealmId,
    },
}

/// A queued job together with the async context it was queued in.
///
/// The context travels *with* the work rather than being read when the work runs, because by
/// then whatever scheduled it has returned: `als.run(store, () => p.then(f))` is finished long
/// before `f` is called, and `f` must still see the store. Capturing at the queue and restoring
/// at the drain is the whole of how a store crosses an `await`.
#[derive(Debug)]
pub(super) struct Scheduled {
    pub(super) context: AsyncContext,
    pub(super) job: Job,
}

/// How a suspended generator is resumed.
///
/// One enum rather than a pair of flags, because the three are mutually exclusive and each is a
/// different thing to do at the suspension point: hand a value back, raise one there, or unwind.
#[derive(Clone, Debug)]
pub(super) enum Resume {
    /// `next(v)`: the value becomes the result of the `yield`.
    Send(Value),
    /// `throw(e)`, and an `await` whose promise rejected. Raised *at* the suspension point, so
    /// the generator's own `try` blocks see it.
    Throw(Value),
    /// `return(v)`: unwinds through every `finally` without resuming the `yield` at all.
    Return(Value),
}

/// The `target` of a handler the host installed, which no compiler-emitted handler can have.
pub(super) const HOST_HANDLER: usize = usize::MAX;

/// A `catch` or `finally` waiting for an exception.
///
/// The frame depth and stack height are recorded so unwinding can restore the machine to exactly
/// the state the handler expects, however many frames the throw crossed.
#[derive(Debug)]
pub(super) struct Handler {
    pub(super) target: usize,
    pub(super) frame_depth: usize,
    pub(super) stack_height: usize,
    /// How many Rust re-entries deep the machine was when this was installed.
    ///
    /// A built-in that takes a callback -- `map`, `sort`, a promise reaction -- drives the
    /// interpreter from inside a Rust frame of its own. A throw from that callback cannot jump
    /// straight to a handler installed *below* that frame: the Rust driver would still be
    /// sitting there, and would carry on running whatever code the jump landed on. So the
    /// depths are compared, and a throw that would cross one travels as an error instead until
    /// the Rust stack is flat again.
    pub(super) native_depth: usize,
}

/// A suspended caller, restored when the callee returns.
///
/// The module body is not represented here: it is the bottom of the call stack rather than a
/// frame in it, which is why an empty `frames` means `Return` ends execution.
pub(super) struct Frame {
    /// The program that was running, restored with the frame.
    pub(super) program: Rc<Program>,
    /// Its strictness table, restored with it.
    pub(super) strictness: Rc<Strictness>,
    /// Whether the caller was strict code, restored with the frame.
    pub(super) strict: bool,
    /// The module whose lexical code is running, restored with the caller frame.
    pub(super) module_path: Option<Rc<str>>,
    /// The dynamic import generation of this activation's module context.
    pub(super) module_generation: u64,
    pub(super) module_generation_owner: Option<crate::module_map::ModuleGenerationHandle>,
    /// `new.target` in the caller, restored with the frame.
    pub(super) new_target: Value,
    /// The closure that was running, or `None` for the module body.
    pub(super) function: Option<Rc<Closure>>,
    /// Property name through which this activation was called, for V8-compatible stack aliases.
    pub(super) method_name: Option<StringUnits>,
    pub(super) this_value: Value,
    /// What this frame was called with, so `super(...arguments)` can forward them.
    pub(super) arguments: Vec<Value>,
    /// The `arguments` object this frame handed out, restored with the frame.
    pub(super) arguments_object: Option<Value>,
    /// Whether the caller's `arguments` is the mapped kind, restored with the frame.
    pub(super) mapped_arguments: bool,
    /// The instance being built, when this frame is a `new` call.
    ///
    /// Held here rather than in a parallel stack so that `Return` can decide locally: a
    /// constructor yields its instance unless the body returned an object, which wins.
    pub(super) constructing: Option<Value>,
    /// Restore the caller with the object produced by super(), without a nested Rust call.
    pub(super) super_call: bool,
    pub(super) locals: Vec<Value>,
    /// The realm the caller was running in, restored with the frame.
    ///
    /// A call into a function from another realm *is* a realm switch: everything the callee
    /// mints -- an intrinsic it reads off a table, a `TypeError` it throws, an object it
    /// constructs -- belongs to its realm and not to the caller's.
    pub(super) realm: RealmId,
    /// Which frame's locals were module scope for the code that pushed this one.
    ///
    /// Every ordinary call inherits it, because a callee shares its module's bindings. An eval
    /// does not: its program has module-scope slots of its own, and reading them out of the
    /// caller's module frame would find unrelated bindings by number.
    pub(super) global_base: usize,
    /// Where to resume in the caller.
    pub(super) instruction: usize,
    /// Stack height when the call began, so a callee that leaves debris cannot corrupt the
    /// caller's operand stack.
    pub(super) stack_base: usize,
}

#[derive(Clone, Copy)]
pub(super) enum HostResultConversion {
    Direct,
    Utf8,
    DirEntries { with_file_types: bool },
}

pub(super) struct PendingHostRequest {
    pub(super) request: RequestId,
    pub(super) promise: Rc<PromiseData>,
    pub(super) conversion: HostResultConversion,
}

pub(super) enum PendingNodeRequest {
    Server {
        request: RequestId,
        state: Rc<RefCell<NodeServerState>>,
    },
    Stream {
        request: RequestId,
        server: Rc<RefCell<NodeServerState>>,
        resource: kiln_host_abi::ResourceId,
        is_write: bool,
        write_callbacks: Vec<Value>,
    },
    Connect {
        request: RequestId,
        target: PendingConnect,
    },
    CloseResource {
        request: RequestId,
    },
}

pub(super) enum PendingConnect {
    Net(Rc<RefCell<NetSocketState>>),
    Http(Rc<RefCell<HttpClientRequestState>>),
}

impl PendingNodeRequest {
    pub(super) fn request(&self) -> RequestId {
        match self {
            Self::Server { request, .. }
            | Self::Stream { request, .. }
            | Self::Connect { request, .. }
            | Self::CloseResource { request } => *request,
        }
    }
}

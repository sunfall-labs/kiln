//! A small, deterministic stack interpreter for Kiln's parser-independent IR.
//!
//! The interpreter deliberately knows nothing about the frontend.  It consumes only the owned
//! bytecode in `kiln-ir` and reaches the outside world through the versioned `HostOps` trait. The
//! VM owns values, objects, exceptions and realm-local execution; `kiln-runtime` supplies host
//! scheduling, and `kiln-module-eval` coordinates linked module graph execution.

use kiln_host_abi::{
    ActiveAsyncHostOps, AsyncHostOp, AsyncHostValue, Completion, HostOp, HostOps, HostValue,
    RequestId,
};
use kiln_ir::{
    BinaryOp, Constant, EvalAccess, EvalSite, Instruction, ModuleProgram, PrivateKind, Program,
    Strictness, UnaryOp, Upvalue,
};

use kiln_runtime::{
    ActiveHttpRuntime, ClockCapability, DEFAULT_ENTROPY_SEED, EntropyCapability, HttpCloseMode,
    LocaleCapability, Realm, RealmCapabilities, RealmRuntime, RuntimeHttpClientEvent,
    RuntimeHttpClientRequest, RuntimeHttpEvent, RuntimeTurn, TimeZoneCapability, TimerId, WorkItem,
    duration_from_milliseconds,
    node_http::HttpResponseEvent,
    node_http_agent::{AgentCancellation, AgentDecision, AgentFailure, AgentRelease, HttpOrigin},
    node_net::{ServerAction, TcpServerDriver},
    node_stream::{DEFAULT_HIGH_WATER_MARK, StreamAction, StreamDriver, StreamEvent},
};

// `Temporal` is glue over a linked calendar engine, and it is long enough that inlining a ninth
// built-in family here would bury the machine. ADR 0025 links the engine; ADR 0029 §4 names it.
mod temporal;
// `Intl` is glue over the same kind of linked engine, and its option-reading half is long
// enough to bury the machine for the same reason. ADR 0025 links it; ADR 0029 §4 names ICU4X.
mod intl;
// A second agent is a thread, a realm and a turn between them, and none of that is the bytecode
// machine's business until `Atomics.wait` blocks. Kept beside the machine rather than inside it
// so the scheduler can be read -- and reasoned about for determinism -- on its own.
mod agent;
use agent::{AgentGroup, AgentOwner, Bytes};
// `crypto` houses node:crypto and Web Crypto primitives (hashes, HMAC, entropy, constant-time compare).
mod compression;
mod crypto;
#[cfg(test)]
pub(crate) use crypto::{node_hash, node_hash_digest, node_hash_state};
// `zlib` houses node:zlib compression and streaming transforms.
mod zlib;
// `os` houses node:os environment facts and hardware inspection.
mod os;
pub use os::OsFact;
// `path` houses node:path POSIX path utilities.
mod path;
pub use path::PathPart;
// `querystring` houses node:querystring parsing and decoding.
mod querystring;
// `url` houses WHATWG URL and node:url implementation.
mod url;
pub(crate) use url::UrlState;
// `string_decoder` houses node:string_decoder implementation.
mod string_decoder;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::rc::{Rc, Weak};
use std::sync::Arc;
pub(crate) use string_decoder::StringDecoderState;
#[cfg(test)]
pub(crate) use string_decoder::decode_utf8_chunk;
// `util` houses node:util formatting, inspection, and inheritance helpers.
mod util;
pub(crate) use util::format as node_util_format;
// `diagnostics_channel` houses node:diagnostics_channel diagnostic and tracing channels.
mod diagnostics_channel;
// `assert` houses node:assert assertion primitives.
mod assert;
// `tty` houses node:tty terminal detection primitives.
mod tty;
// `dns` houses node:dns deterministic resolution primitives.
mod dns;
// `perf_hooks` houses node:perf_hooks performance hooks namespace.
mod perf_hooks;
// `events` houses node:events and EventEmitter listener tracking.
mod events;
pub(crate) use events::{
    NodeEventBucket, NodeEventListener, node_event_emitter, node_listeners,
    remove_last_node_listener, remove_node_listener,
};
// `stream` houses node:stream Readable and Writable implementations.
mod stream;
pub(crate) use stream::{
    ReadableState, WritableState, new_readable_state, node_readable, node_readable_state,
    node_writable, node_writable_state, sync_readable_public_state, sync_writable_public_state,
};
// `net` houses node:net TCP server and socket implementations.
mod net;
pub(crate) use net::{
    NetSocketState, NodeConnection, NodeServerState, ServerProtocol, node_listen_arguments,
    node_net_socket_state, node_net_socket_value, node_server_object, node_socket_object,
    sync_node_socket_ready_state,
};
// `http` houses node:http, node:https, and node:http2 implementations.
mod http;
pub(crate) use http::{
    HttpClientRequestState, HttpIncomingState, HttpResponseState, node_body_bytes,
    node_http_client_request, node_http_client_request_object, node_http_incoming,
    node_http_request_arguments, node_http_response, node_http_server, readable_chunk_length,
};
// `fs` houses node:fs and node:fs/promises implementations.
mod fs;
pub(crate) use fs::node_encoding;
// `async_hooks` houses node:async_hooks, AsyncLocalStorage, and AsyncResource.
mod async_hooks;
pub(crate) use async_hooks::{
    AsyncContext, AsyncStorage, bind_store, node_async_local_storage, node_async_storage,
};
mod object_model;
pub use object_model::*;
mod gc_census;
pub(crate) use gc_census::tracked_object;
mod shared;
use shared::SharedCache;
pub use shared::SharedFacade;
mod vm;
use vm::Vm;
mod intrinsics;
use intrinsics::call_native;
mod errors;
pub use errors::*;
// `buffer` houses node:buffer, Buffer constructor and statics.
mod buffer;
pub(crate) use buffer::node_bytes_value;
// `console` houses console methods and node:console.
mod console;
pub use kiln_string::{StringUnits, StringView};

/// The built-ins the runtime provides natively.
///
/// Kept minimal on purpose. design.md §5 wants built-ins written in JavaScript, and everything
/// here is a placeholder for one -- but nothing can bootstrap a JS built-in until there are
/// prototypes and a loader, so the handful the conformance corpus needs first live here.
mod intrinsic_members;
mod symbol;
pub use symbol::*;

/// The reservation a private name is stored under, ahead of its scope number and its spelling.
///
/// A private element is not a property a program can name, so it is kept under a key no source
/// text can spell -- which means every message and every `name` that has to *show* one has to
/// undo the reservation rather than print it.
const PRIVATE_KEY_PREFIX: &str = "@@private:";

/// The error constructors a realm provides.
///
/// One enum rather than one `Native` each, because everything about them is the same shape: a
/// name, a prototype that chains to `Error.prototype`, and a constructor reachable from an
/// instance through that prototype. The VM also raises them itself, and having the kinds in one
/// type is what lets it say *which* without a second table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Error,
    Type,
    Range,
    Reference,
    Syntax,
    Uri,
    /// `EvalError`, which nothing in the language throws any more -- `eval` stopped raising it
    /// in ES5. It is still a realm intrinsic with a prototype and a constructor of its own, and
    /// the corpus reaches for it constantly as *an* error type that is not one of the others.
    Eval,
    /// `SuppressedError`, which explicit resource management raises when disposing one resource
    /// throws while another throw is already travelling. It is an error kind rather than a type
    /// of its own because that is exactly what it is: an `Error` with two extra fields naming
    /// the two throws, and a prototype chaining to `Error.prototype` like every other.
    Suppressed,
    /// `AggregateError`, which reports several failures as one. `Promise.any` raises it when
    /// every input rejected, and it is the same shape as `SuppressedError`: an `Error` with one
    /// extra field -- `errors` -- and a prototype chaining to `Error.prototype`.
    Aggregate,
}

impl ErrorKind {
    /// Every kind, in declaration order, so the realm can build all their prototypes.
    const ALL: [Self; 9] = [
        Self::Error,
        Self::Type,
        Self::Range,
        Self::Reference,
        Self::Syntax,
        Self::Uri,
        Self::Eval,
        Self::Suppressed,
        Self::Aggregate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Type => "TypeError",
            Self::Range => "RangeError",
            Self::Reference => "ReferenceError",
            Self::Syntax => "SyntaxError",
            Self::Uri => "URIError",
            Self::Eval => "EvalError",
            Self::Suppressed => "SuppressedError",
            Self::Aggregate => "AggregateError",
        }
    }

    /// Which argument is the message.
    ///
    /// Not the first for every kind. `SuppressedError(error, suppressed, message)` puts it third
    /// and `AggregateError(errors, message)` puts it second, both for the same reason: the
    /// failures the type is reporting are what it is *for*, and the sentence is an afterthought.
    const fn message_index(self) -> usize {
        match self {
            Self::Suppressed => 2,
            Self::Aggregate => 1,
            _ => 0,
        }
    }

    /// The own properties this kind carries beside `stack` and `message`, in argument order.
    ///
    /// Each is taken from the argument at its own position and installed whatever was passed,
    /// `undefined` included: a program that catches one of these reads the field without
    /// checking first, and an absent property would make that read a silent `undefined` from a
    /// prototype rather than the value the constructor was handed.
    const fn carried_fields(self) -> &'static [&'static str] {
        match self {
            Self::Suppressed => &["error", "suppressed"],
            Self::Aggregate => &["errors"],
            _ => &[],
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .expect("every kind is in ALL")
    }
}

mod native;
pub use native::*;

mod module_map;
pub use module_map::{ModuleMap, ModuleStatus, SharedModuleMap, lookup_export};

/// Which built-in's method table a `Native::PrototypeOf` marker stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrototypeTable {
    Object,
    Array,
    String,
    Number,
    Function,
    RegExp,
    Date,
    ArrayBuffer,
    /// `SharedArrayBuffer.prototype`. Kept apart from the table above because the two share no
    /// methods a program can observe as shared: `resize` is on one and `grow` on the other, and
    /// each throws when reached with the other's receiver.
    SharedArrayBuffer,
    /// `%TypedArray%.prototype`, where every view's methods actually live.
    TypedArray,
    /// `Int8Array.prototype` and its eight siblings. Kept apart from the table above, and from
    /// each other, because the specification keeps them apart: each holds the one property that
    /// differs between the views -- `BYTES_PER_ELEMENT` -- and a program can see that
    /// `Int8Array.prototype !== Uint8Array.prototype`.
    TypedArrayFor(TypedKind),
    DataView,
    /// `%IteratorPrototype%`, where the iterator helpers live and which every iterator this
    /// realm makes answers from.
    Iterator,
    /// `%GeneratorPrototype%` and `%AsyncGeneratorPrototype%`: `next`, `return` and `throw`, and
    /// the tag. A generator object reaches one of these through its function's `prototype`, which
    /// is the link that made `Object.getPrototypeOf(g())` answer null -- so every test that walks
    /// a generator's chain read `undefined.next` instead of a method the value has answered all
    /// along. Both sit above `%IteratorPrototype%`.
    Generator,
    AsyncGenerator,
    /// `%AsyncIteratorPrototype%`, where `@@asyncIterator` lives.
    AsyncIterator,
    /// `%ArrayIteratorPrototype%` and its four siblings. Each owns `next` and a tag naming the
    /// kind, and each sits above `%IteratorPrototype%` -- so the iterator objects themselves own
    /// nothing, which is what `Object.getOwnPropertyNames` on one reports.
    IteratorFor(IteratorKind),
    /// The three wrapper tables. Each answers to almost nothing of its own -- `toString` and
    /// `valueOf` -- but a program that names one is nearly always about to `call` that method
    /// onto a receiver it supplies, which is precisely what a marker supports.
    Boolean,
    Symbol,
    BigInt,
    Promise,
    /// The four collection tables. The weak pair is kept apart from the strong pair because the
    /// specification gives them different sets of methods: a weak collection has no `size`, no
    /// iteration, and no `forEach`, and answering those from a shared table would hand a program
    /// entries a real engine would already have dropped.
    Map,
    Set,
    WeakMap,
    WeakSet,
    /// The two tables for holding something weakly *without* being a collection of it. Neither
    /// is a `WeakSet` with one member: a `WeakRef` hands its target back, which a weak set
    /// deliberately cannot, and a registry holds a value it is *not* weak about at all.
    WeakRef,
    FinalizationRegistry,
    /// `ShadowRealm.prototype`, holding `evaluate`, `importValue` and a `Symbol.toStringTag`.
    ///
    /// It is *not* where the realm lives: the realm is an internal slot on the instance, so a
    /// method reached off this table with a foreign receiver finds no slot and throws, which is
    /// what stops `evaluate.call({}, src)` from evaluating anything.
    ShadowRealm,
    DisposableStack,
    AsyncDisposableStack,
    EventEmitter,
    Console,
    /// `node:stream`'s readable method table. It inherits EventEmitter and owns no transport:
    /// HTTP, TCP and in-memory producers all feed the same state machine through `push`.
    Readable,
    /// `node:stream`'s writable method table. Like Readable, it owns lifecycle and callback
    /// ordering while a transport adapter decides where accepted chunks ultimately go.
    Writable,
    WebStub(native::WebStubKind),
}

/// Which of the lazy iterator helpers a helper object is running.
///
/// Each of these builds an iterator rather than walking one: the source is pulled from a single
/// element at a time, which is what lets `map` over an endless sequence followed by `take(3)`
/// terminate. An eager implementation of any of them would make that program hang.
/// Which built-in iterator a `%...IteratorPrototype%` belongs to.
///
/// They differ only in their tag and in which `next` they publish, which is exactly why one
/// parameterised table serves all five rather than five tables that would drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IteratorKind {
    Array,
    String,
    Map,
    Set,
    RegExpString,
}

impl IteratorKind {
    /// The tag `Object.prototype.toString` reports through `@@toStringTag`.
    const fn tag(self) -> &'static str {
        match self {
            Self::Array => "Array Iterator",
            Self::String => "String Iterator",
            Self::Map => "Map Iterator",
            Self::Set => "Set Iterator",
            Self::RegExpString => "RegExp String Iterator",
        }
    }

    /// The `next` this kind publishes, which is the one its instances already answered.
    fn next_method(self) -> Value {
        match self {
            Self::Array => Value::Native(Native::ArrayIteratorNext, RealmId::PRIMARY),
            Self::String => Value::Native(Native::StringIteratorNext, RealmId::PRIMARY),
            Self::Map | Self::Set => {
                Value::Native(Native::CollectionIteratorNext, RealmId::PRIMARY)
            }
            Self::RegExpString => Value::Native(Native::RegExpStringIteratorNext, RealmId::PRIMARY),
        }
    }
}

/// What one of the five iterator prototypes answers.
fn iterator_prototype_method(kind: IteratorKind, name: &str) -> Value {
    match name {
        "next" => kind.next_method(),
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String(kind.tag().to_owned().into())
        }
        // Everything else is `%IteratorPrototype%`'s, one link up: the helpers, and `@@iterator`
        // itself. The chain walk follows one marker and stops, so the step is taken here -- the
        // same shape `inherit_from_object` gives every other table for `Object.prototype`.
        // Without it `[..."a".matchAll(re)]` stopped working: `next` resolved and the object was
        // still not iterable, which is a spread refusing an iterator. A table has no store, so
        // this answers only what the table itself holds; the members the shared bootstrap
        // defines there (`chunks`, `join`, ...) are found by the reads that can consult it
        // (`iterator_prototype_property`).
        other => iterator_method(other),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    Map,
    Filter,
    Take,
    Drop,
    FlatMap,
}

impl HelperKind {
    /// The name the method is bound under, which is also the `name` its function reports.
    const fn property_key(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Filter => "filter",
            Self::Take => "take",
            Self::Drop => "drop",
            Self::FlatMap => "flatMap",
        }
    }
}

/// Which of the iterator methods that *consume* an iterator is running.
///
/// The division is the specification's and it is not cosmetic: these return a value rather than
/// another iterator, so each one drives the source to its end -- or to the element that answers
/// the question, at which point it closes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalKind {
    Reduce,
    ToArray,
    ForEach,
    Some,
    Every,
    Find,
    /// `includes(x)`: whether the source ever produces `x`, by `SameValueZero`.
    Includes,
}

impl TerminalKind {
    /// The name the method is bound under, which is also the `name` its function reports.
    const fn property_key(self) -> &'static str {
        match self {
            Self::Reduce => "reduce",
            Self::ToArray => "toArray",
            Self::ForEach => "forEach",
            Self::Some => "some",
            Self::Every => "every",
            Self::Find => "find",
            Self::Includes => "includes",
        }
    }
}

/// The iterator a helper pulls from. Its presence is also what marks an object as one of this
/// realm's iterators, which is how `map` and its siblings are found on it and how
/// `instanceof Iterator` answers.
const HELPER_SOURCE: &str = "@@helper-source";
/// The source's `next`, read once when the helper was built. A program that replaces `next`
/// afterwards keeps stepping through the one the helper captured, which is what the
/// specification says and what a test counting the property reads is watching.
const HELPER_NEXT: &str = "@@helper-next";
/// The callback, or -- for `take` and `drop` -- the count still to go.
const HELPER_ARG: &str = "@@helper-arg";
/// The index handed to the callback as its second argument.
const HELPER_COUNTER: &str = "@@helper-counter";
/// Latched once the helper has finished, so a second `next` answers done rather than reaching
/// back into a source that is exhausted or broken.
const HELPER_DONE: &str = "@@helper-done";
/// `flatMap`'s current inner iterator, and the `next` it was captured with.
/// The regexp `@@matchAll`'s iterator walks with -- a clone of the receiver, so the program's
/// own regexp keeps its cursor.
const MATCH_ALL_REGEXP: &str = "@@matchall-regexp";
const MATCH_ALL_SUBJECT: &str = "@@matchall-subject";
const MATCH_ALL_GLOBAL: &str = "@@matchall-global";
const MATCH_ALL_UNICODE: &str = "@@matchall-unicode";

const HELPER_INNER: &str = "@@helper-inner";
const HELPER_INNER_NEXT: &str = "@@helper-inner-next";

/// One reserved slot of a helper.
fn helper_slot(state: &Rc<RefCell<ObjectData>>, key: &str) -> Value {
    state
        .borrow()
        .get_own(key)
        .cloned()
        .unwrap_or(Value::Undefined)
}

/// One operation of Test262's agent protocol, as the interpreter publishes it.
///
/// The names are the hooks' names -- `$262AgentStart` and its seven siblings -- because a hook
/// is a name a program reaches for, and the C backend refuses them by exactly that name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentOp {
    Start,
    Broadcast,
    GetReport,
    Sleep,
    MonotonicNow,
    ReceiveBroadcast,
    Report,
    Leaving,
}

impl AgentOp {
    /// Every hook, in the order the host object lists them.
    pub const ALL: [Self; 8] = [
        Self::Start,
        Self::Broadcast,
        Self::GetReport,
        Self::Sleep,
        Self::MonotonicNow,
        Self::ReceiveBroadcast,
        Self::Report,
        Self::Leaving,
    ];

    /// The global name this hook is published under.
    #[must_use]
    pub const fn hook_name(self) -> &'static str {
        match self {
            Self::Start => "$262AgentStart",
            Self::Broadcast => "$262AgentBroadcast",
            Self::GetReport => "$262AgentGetReport",
            Self::Sleep => "$262AgentSleep",
            Self::MonotonicNow => "$262AgentMonotonicNow",
            Self::ReceiveBroadcast => "$262AgentReceiveBroadcast",
            Self::Report => "$262AgentReport",
            Self::Leaving => "$262AgentLeaving",
        }
    }

    const fn arity(self) -> f64 {
        match self {
            Self::GetReport | Self::MonotonicNow | Self::Leaving => 0.0,
            _ => 1.0,
        }
    }
}

/// One operation the `Atomics` namespace publishes.
///
/// One variant with a payload rather than fourteen flat ones, for the reason `Temporal` has
/// one: every read-modify-write shares a validation sequence whose *order* is observable, and
/// only what it does with the two values it has differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicsOp {
    Add,
    And,
    Or,
    Xor,
    Sub,
    Exchange,
    CompareExchange,
    Load,
    Store,
    IsLockFree,
    Pause,
    Wait,
    WaitAsync,
    Notify,
}

impl AtomicsOp {
    /// Every operation, in the order `Atomics` publishes them.
    const ALL: [Self; 14] = [
        Self::Add,
        Self::And,
        Self::CompareExchange,
        Self::Exchange,
        Self::IsLockFree,
        Self::Load,
        Self::Or,
        Self::Pause,
        Self::Store,
        Self::Sub,
        Self::Wait,
        Self::WaitAsync,
        Self::Notify,
        Self::Xor,
    ];

    const fn property_key(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Sub => "sub",
            Self::Exchange => "exchange",
            Self::CompareExchange => "compareExchange",
            Self::Load => "load",
            Self::Store => "store",
            Self::IsLockFree => "isLockFree",
            Self::Pause => "pause",
            Self::Wait => "wait",
            Self::WaitAsync => "waitAsync",
            Self::Notify => "notify",
        }
    }

    const fn arity(self) -> f64 {
        match self {
            Self::IsLockFree | Self::Pause => 1.0,
            Self::Load => 2.0,
            Self::Add
            | Self::And
            | Self::Or
            | Self::Xor
            | Self::Sub
            | Self::Exchange
            | Self::Store
            | Self::Notify => 3.0,
            Self::CompareExchange | Self::Wait | Self::WaitAsync => 4.0,
        }
    }

    /// Whether this operation may only be applied to a view a program can wait on.
    ///
    /// `Int32Array` and `BigInt64Array` and nothing else: waiting is defined in terms of a
    /// waiter list keyed by a *word* in a buffer, and the specification only gives one for
    /// those two widths.
    const fn waitable(self) -> bool {
        matches!(self, Self::Wait | Self::WaitAsync | Self::Notify)
    }
}

/// A proxy's two halves.
///
/// Both are held as ordinary values: a handler is a plain object whose properties are the traps,
/// and a target is whatever was proxied -- including another proxy, which is why neither is
/// narrowed to a type here.
#[derive(Debug, PartialEq)]
pub struct ProxyData {
    target: Value,
    handler: Value,
    /// Private elements belong to the proxy object itself. They are internal slots, so class
    /// initialization and `#name` access bypass the public proxy traps and never touch `target`.
    private_elements: RefCell<ObjectData>,
    /// `Proxy.revocable`'s second half was called.
    ///
    /// A revoked proxy is not an empty one: every operation on it is a TypeError, which is the
    /// entire point -- the capability it represented has been withdrawn, and a program holding a
    /// stale reference has to find out rather than see a plausible answer.
    revoked: std::cell::Cell<bool>,
}

/// What `bind` captured: the function to run, the receiver to run it with, and the arguments to
/// put in front of the call's own.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundFunction {
    target: Value,
    this_value: Value,
    arguments: Vec<Value>,
    /// Own properties of this callable wrapper.
    ///
    /// A bound function is still an ordinary ECMAScript object. Packages routinely attach
    /// metadata to callbacks after binding them, so keeping these properties in the realm-wide
    /// intrinsic override table would conflate distinct bound-function identities.
    properties: RefCell<ObjectData>,
}

/// A wrapped function exotic object: the only thing a `ShadowRealm` ever hands across the
/// boundary that is not a primitive.
///
/// The whole point of the type is that it is *not* `target`. `target` is a callable belonging to
/// another realm, and handing it out directly would be the silently wrong answer this exists to
/// prevent: the caller would hold a function whose `[[Prototype]]`, whose properties and whose
/// identity all came from a realm it is not supposed to be able to reach. So the wrapper lives in
/// `realm` -- the *caller's* -- carries only the `name` and `length` copied off the target, and
/// forwards a call by wrapping every argument on the way in and the result on the way out.
///
/// `realm` is stored rather than derived from `target` for the same reason: `GetFunctionRealm` of
/// a wrapped function is the realm that *made the wrapper*, so a `TypeError` it throws is the
/// caller's `TypeError` and not the target realm's. Deriving it from the target would answer that
/// question with the one realm the caller must not see.
#[derive(Debug)]
pub struct WrappedFunction {
    /// The callable in the other realm that this forwards to.
    ///
    /// Never handed out, never compared against, and never reachable from JavaScript: the only
    /// thing that reads it is the call path, which enters `target`'s realm to run it.
    target: Value,
    /// The realm this wrapper belongs to, which is the caller realm at the moment it was made.
    realm: RealmId,
    /// `name` and `length`, copied off the target when the wrapper was made, plus whatever the
    /// holder puts on it afterwards.
    ///
    /// Copied rather than forwarded because the specification copies: `CopyNameAndLength` reads
    /// both once, at wrap time, so a later write to the target's `name` is not visible here --
    /// and a property the *holder* adds must not be visible to the target realm either. That is
    /// what "share no properties" means, and forwarding either direction would break it.
    properties: RefCell<ObjectData>,
}

/// A generator owns ordinary properties independently of its suspended execution state.
/// Keeping the stores separate lets property access work while its frame is running.
#[derive(Debug)]
pub struct GeneratorData {
    state: RefCell<GeneratorState>,
    properties: RefCell<ObjectData>,
}

impl PartialEq for GeneratorData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl std::ops::Deref for GeneratorData {
    type Target = RefCell<GeneratorState>;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// A generator's saved execution state.
///
/// A generator is the first thing in this VM whose frame does not run to completion, so its
/// registers live here between resumptions rather than on the frame stack. Resuming installs
/// them, running suspends back into them, and the operand stack is saved alongside because a
/// `yield` can occur part-way through an expression.
#[derive(Debug)]
pub struct GeneratorState {
    closure: Rc<Closure>,
    locals: Vec<Value>,
    /// The generator's own operand stack, relative to where it was suspended.
    stack: Vec<Value>,
    instruction: usize,
    this_value: Value,
    new_target: Value,
    arguments: Vec<Value>,
    /// Handlers registered inside the generator, held while it is suspended.
    ///
    /// Stored as offsets rather than absolute positions: a generator's operand stack is placed
    /// wherever the resuming caller's stack happens to end, so a handler recorded during one
    /// resumption points at the wrong place during the next. That is only visible when the two
    /// resumptions happen at different stack depths -- `JSON.stringify(it.return(x))` is such a
    /// case, and is what found it.
    handlers: Vec<(usize, usize, usize)>,
    /// False until the first `next`, which starts the body rather than resuming a `yield`.
    started: bool,
    done: bool,
    /// Set while the body is on the stack, so a resumption from inside it can be refused.
    running: bool,
    /// Why the body suspended: an `await` rather than a `yield`.
    ///
    /// Meaningless for a plain generator, which only ever yields, and for an async function,
    /// which only ever awaits. An async generator does both, and its driver has to know whether
    /// the consumer's step is finished or whether it is still waiting on something.
    awaiting: bool,
    /// The value a `.return()` is unwinding towards, while that unwind is *suspended*.
    ///
    /// `.return()` unwinds by throwing a marker, and the marker is recognised by the resumption
    /// that threw it. An async generator can suspend part-way through that unwind -- a `finally`
    /// block is ordinary code and may `await` -- and it is then resumed by the driver with an
    /// ordinary `Send`, which knows nothing about a return in flight. Without this the marker
    /// reached the driver as though it were a thrown value, and the consumer got a promise
    /// rejected with `[Function: returnSignal]`: not a JavaScript value at all, from a program
    /// that had merely written `break` inside `for await`.
    returning: Option<Value>,
    /// When suspended at `YieldStar`, the active delegate iterator, the target instruction to resume
    /// at if `inner.throw()` completes with `done: true`, and whether the delegate is a wrapped synchronous iterator.
    delegate: Option<(Value, usize, bool)>,
    /// Async-generator requests are serialized: only the first request may resume the body, and
    /// the next request starts after the current request's result is settled.
    async_requests: Option<Box<AsyncGeneratorRequests>>,
}

#[derive(Debug, Default)]
pub(crate) struct AsyncGeneratorRequests {
    pub(crate) current: Option<AsyncGeneratorRequest>,
    pub(crate) pending: std::collections::VecDeque<AsyncGeneratorRequest>,
}

#[derive(Debug)]
pub(crate) struct AsyncGeneratorRequest {
    pub(crate) kind: AsyncGeneratorRequestKind,
    pub(crate) value: Value,
    pub(crate) step: Rc<PromiseData>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AsyncGeneratorRequestKind {
    Next,
    Return,
    Throw,
}

impl GeneratorState {
    /// Mark the generator completed and release values that are only needed while it can run.
    ///
    /// The generator object keeps its closure and own properties after completion, but its
    /// suspended frame is no longer reachable by a future `next`, `return`, or `throw` call.
    /// Keeping that frame here would retain every local and argument for as long as callers keep
    /// the completed iterator alive.
    pub(crate) fn complete(&mut self) {
        self.done = true;
        self.locals.clear();
        self.stack.clear();
        self.arguments.clear();
        self.handlers.clear();
        self.this_value = Value::Undefined;
        self.new_target = Value::Undefined;
        self.awaiting = false;
        self.returning = None;
        self.delegate = None;
    }
}

impl PartialEq for GeneratorState {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

#[derive(Debug)]
struct ModuleSuspension {
    stack: Vec<Value>,
    handlers: Vec<Handler>,
    instruction: usize,
    module_generation: Option<module_map::ModuleGenerationHandle>,
    completion: Rc<PromiseData>,
    nested: Option<SuspendedModuleContext>,
}

/// The module activation that must be parked when a dynamically imported module reaches TLA.
/// Unlike an async function, a module body is the machine's active program rather than a
/// `GeneratorData`; preserve its program and lexical slots as well as the instruction offset.
#[derive(Debug)]
struct SuspendedModuleContext {
    program: Rc<Program>,
    strictness: Rc<Strictness>,
    strict: bool,
    module_path: Option<Rc<str>>,
    module_generation: u64,
    module_generation_owner: Option<module_map::ModuleGenerationHandle>,
    locals: Vec<Value>,
    self_bindings: Vec<(Cell, u32)>,
    self_namespaces: Vec<SelfNamespace>,
    realm: RealmId,
    target: ModuleEvaluationTarget,
}

#[derive(Debug)]
struct ModuleEvaluationTarget {
    key: String,
    exports: Vec<String>,
    reexports: Vec<(String, ModuleBinding)>,
    private_window: Option<PrivateWindow>,
}

impl PartialEq for ModuleSuspension {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// Clears a generator's `running` flag however its resumption ends.
///
/// It has to come off on every path -- a suspension, a return, an unwind, or a `?` out of the
/// middle of `resume_generator_with` -- because a generator left marked as executing would
/// refuse every later `next()`.
struct RunningGuard<'a>(&'a Rc<GeneratorData>);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().running = false;
    }
}

/// A callable value: which function body to run, and the bindings it closed over.
#[derive(Debug)]
pub struct Closure {
    /// The program this closure's code lives in.
    ///
    /// `function` indexes *this* table, not the running VM's. A function value that crosses a
    /// module boundary -- which is what `export function` produces -- would otherwise be called
    /// against the importer's function table and find someone else's body, or nothing at all.
    pub program: Rc<Program>,
    pub function: u32,
    /// Which of `program`'s bodies are strict code.
    ///
    /// Carried with the program for the same reason `program` is: a function value that crossed
    /// a module boundary is indexed against its own table, and answering from the caller's would
    /// report someone else's body. Whether *this* body is strict is `strictness.function`.
    pub strictness: Rc<Strictness>,
    /// The module whose lexical code created this closure, used to resolve later import().
    pub module_path: Option<Rc<str>>,
    /// Dynamic import generation captured with the module context that created this closure.
    pub module_generation: u64,
    /// Identity-bearing owner for the captured generation. Interior mutability lets the cycle
    /// collector drop this edge after proving an otherwise unreachable closure/epoch cycle.
    pub(crate) module_generation_owner: RefCell<Option<module_map::ModuleGenerationHandle>>,
    /// The declared name, for a console to show. Shared with every other closure over the same
    /// proto rather than copied, because a function expression in a loop makes one closure per
    /// pass and the name is the same every time.
    pub name: Option<Rc<str>>,
    /// A lossless display name supplied by named evaluation for a function whose source had no
    /// written identifier. This stays separate from `name`: the latter is a scalar binding.
    pub inferred_name: Option<StringUnits>,
    /// One cell per [`kiln_ir::FunctionProto::upvalues`] entry, in the same order.
    pub upvalues: Vec<Cell>,
    /// The body has a graph proof that each `LoadArguments` is used only for `.length`.
    /// This authorizes the paired-instruction fast path without changing the boxed fallback.
    pub(crate) arguments_length_only: bool,
    /// The realm this function was created in.
    ///
    /// `GetFunctionRealm`, and the only thing that makes it observable: a constructor whose
    /// `prototype` is not an object falls back to *its own* realm's intrinsic prototype, so a
    /// `new.target` from another realm decides what the built-in it is passed to produces.
    pub realm: RealmId,
    /// The enclosing function's `arguments`, captured for an arrow.
    ///
    /// An arrow has no `arguments` of its own; it sees the enclosing function's, from where it
    /// was written. Captured alongside `this` because the rule is the same one -- and captured as
    /// the *object*, not as a copy of the list: `arguments` names one object per call, and an
    /// arrow that copied it would hand back something the enclosing function could not see a
    /// write to.
    pub captured_arguments: Option<Value>,
    /// `new.target` captured where the closure was written, for an arrow function.
    ///
    /// An arrow has no `new.target` of its own for the same reason it has no `this`.
    pub captured_new_target: Option<Value>,
    /// `this` captured where the closure was written, for an arrow function.
    ///
    /// An arrow has no `this` of its own: it uses the one from its defining scope, permanently.
    /// Capturing it here rather than at the call is the whole difference between an arrow and an
    /// ordinary function, and it is why `arrow` has to survive lowering.
    pub captured_this: Option<Value>,
    /// The object `new` gives instances of this function as their prototype.
    ///
    /// Lazily installed: an ordinary function never used as a constructor never allocates one.
    pub prototype: RefCell<Option<Value>>,
    /// Own properties of the function itself, which is where static methods live.
    ///
    /// A function is an object in JavaScript, and a class's statics are properties of the
    /// constructor. Without this, `class C { static f() {} }` would have nowhere to put `f`.
    pub properties: RefCell<ObjectData>,
}

/// A shared, mutable binding.
///
/// Captured variables are shared rather than copied because JavaScript closes over the binding,
/// not the value: two closures over the same `let` must observe each other's writes, and so must
/// the enclosing function after they run.
pub type Cell = Rc<RefCell<Value>>;

/// Runtime hook used by a CommonJS `require` call to evaluate a not-yet-loaded target.
///
/// The graph owns the loader and cache policy; the interpreter only invokes this hook at the
/// point the JavaScript call actually occurs. Keeping the hook outside [`CommonJsRequire`] means
/// a statically discovered edge does not become an eager execution obligation.
pub trait CommonJsModuleLoader {
    /// Receives a thrown value from a lazily evaluated target without putting the realm-local
    /// value into the cross-thread `ExecutionError` report type.
    fn record_thrown_value(&mut self, value: Value);

    /// Takes the value recorded while evaluating the most recent lazy target.
    fn take_thrown_value(&mut self) -> Option<Value>;

    fn get_builtin(
        &mut self,
        specifier: &str,
        instruction: usize,
    ) -> Result<Option<Value>, ExecutionError>;

    fn create_require(
        &mut self,
        importer: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError>;

    fn load(
        &mut self,
        require: &CommonJsRequire,
        specifier: &str,
        instruction: usize,
    ) -> Result<Value, ExecutionError>;

    /// The body of the module at `module_path` has run to its end (or its top-level `await` has
    /// settled), and `exports` are its own bindings. Synchronous bodies publish before queued
    /// jobs run; suspended bodies publish before their module-record completion is resolved.
    fn module_evaluated(&mut self, _module_path: &str, _exports: &[ModuleExportValue]) {}

    /// Materialize an already evaluated CommonJS module's ESM namespace only when ESM linking
    /// needs it. A require-only execution must not invoke exported property getters.
    fn prepare_module_namespace(
        &mut self,
        _key: &str,
        _instruction: usize,
    ) -> Result<(), ExecutionError> {
        Ok(())
    }

    /// Evaluate the CommonJS or JSON module `key` into the realm's module map, as `import()`
    /// of it needs, through the same cache `require` uses. A loader that cannot answers with
    /// the refusal it has always given.
    fn evaluate_module(&mut self, key: &str, _instruction: usize) -> Result<(), ExecutionError> {
        Err(ExecutionError::DynamicImportUnsupported {
            message: format!("`import()` of `{key}` needs a CommonJS loader"),
        })
    }
}

/// One statically resolved CommonJS `require` environment.
///
/// Targets are cells rather than copied values so insertion-before-evaluation and reassignment
/// of `module.exports` have the same identity semantics as Node's module cache.
#[derive(Debug, PartialEq)]
pub struct CommonJsRequire {
    targets: Vec<(String, String, Cell)>,
    resolutions: Vec<(String, String)>,
    importer: Option<String>,
}

impl CommonJsRequire {
    pub fn new(targets: impl IntoIterator<Item = (String, String, Cell)>) -> Self {
        Self {
            targets: targets.into_iter().collect(),
            resolutions: Vec::new(),
            importer: None,
        }
    }

    pub fn for_importer(importer: impl Into<String>) -> Self {
        Self {
            targets: Vec::new(),
            resolutions: Vec::new(),
            importer: Some(importer.into()),
        }
    }

    pub fn for_module(
        importer: impl Into<String>,
        targets: impl IntoIterator<Item = (String, String, Cell)>,
    ) -> Self {
        Self {
            targets: targets.into_iter().collect(),
            resolutions: Vec::new(),
            importer: Some(importer.into()),
        }
    }

    /// Add importer-scoped literal resolver answers without turning them into eager `require`
    /// targets. `require.resolve` observes the selected filename but never evaluates the module.
    pub fn with_resolutions(
        mut self,
        resolutions: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.resolutions = resolutions.into_iter().collect();
        self
    }

    pub fn importer(&self) -> Option<&str> {
        self.importer.as_deref()
    }

    fn get(&self, specifier: &str) -> Option<Value> {
        self.targets
            .iter()
            .find(|(candidate, _, _)| candidate == specifier)
            .map(|(_, _, value)| value.borrow().clone())
    }

    fn resolve(&self, specifier: &str) -> Option<String> {
        self.targets
            .iter()
            .find(|(candidate, _, _)| candidate == specifier)
            .map(|(_, target, _)| target.clone())
            .or_else(|| {
                self.resolutions
                    .iter()
                    .find(|(candidate, _)| candidate == specifier)
                    .map(|(_, target)| target.clone())
            })
    }

    /// Return the cache cell selected by one statically discovered specifier.
    pub fn target_cell(&self, specifier: &str) -> Option<Cell> {
        self.targets
            .iter()
            .find(|(candidate, _, _)| candidate == specifier)
            .map(|(_, _, value)| Rc::clone(value))
    }

    /// Return the currently cached value for a statically discovered specifier.
    pub fn cached_value(&self, specifier: &str) -> Option<Value> {
        self.get(specifier)
    }
}

impl Closure {
    /// The display name exposed through function metadata, preserving UTF-16 code units.
    ///
    /// An empty written name is no written name (anonymous class constructors
    /// carry `Some("")`), so it defers to the inferred name like `None` does.
    pub fn display_name_units(&self) -> StringUnits {
        self.name
            .as_deref()
            .map(StringUnits::from)
            .filter(|units: &StringUnits| !units.is_empty())
            .or_else(|| self.inferred_name.clone())
            .unwrap_or_default()
    }

    /// Whether this body is strict code.
    ///
    /// A property of the source text the body was written in, not of the call: a strict function
    /// called from sloppy code is still strict, and a sloppy one called from strict code is not.
    pub fn is_strict(&self) -> bool {
        self.strictness.function(self.function)
    }

    /// Whether a call of this closure gets the *mapped* `arguments` object.
    ///
    /// Read off the proto rather than recomputed, because it takes both the strictness and the
    /// shape of the parameter list to decide and only the compiler saw the second.
    fn has_mapped_arguments(&self) -> bool {
        self.program
            .functions
            .get(self.function as usize)
            .is_some_and(|proto| proto.mapped_arguments)
    }

    /// The text this function was written as, which is what `Function.prototype.toString` answers.
    ///
    /// The source slice as JavaScript UTF-16 units. The program and its optional parser-origin
    /// mapping are shared across closures; this materializes only the requested slice. The
    /// closure already carries the program for the reason documented on `program` -- a function
    /// value that crossed a module boundary must be read against its own tables.
    ///
    /// A malformed proto index or scalar-source span retains the historical empty fallback. If
    /// an origin is present, however, its map is the only lossless authority: a bad span is a
    /// compiler invariant violation and must not silently return the escaped parser transport.
    pub fn source_units(&self) -> StringUnits {
        let Some(proto) = self.program.functions.get(self.function as usize) else {
            return StringUnits::default();
        };
        if let Some(origin) = &self.program.source_origin {
            return origin
                .slice_bytes(proto.span.start, proto.span.end)
                .expect("compiler function span must resolve in its retained source origin");
        }
        StringUnits::from(
            self.program
                .source
                .get(proto.span.start as usize..proto.span.end as usize)
                .unwrap_or(""),
        )
    }

    /// Lossy host/diagnostic spelling without paying the UTF-16 slice cost for ordinary source.
    pub(crate) fn source_display_text(&self) -> String {
        if self.program.source_origin.is_none() {
            self.source_text().to_owned()
        } else {
            self.source_units().to_lossy_string()
        }
    }

    /// Parser-source spelling for metadata and diagnostics that require Rust `str`.
    /// Language reflection should use [`Self::source_units`] so a lossless UTF-16 origin is
    /// preserved when the parser transport escaped lone surrogate code units.
    pub fn source_text(&self) -> &str {
        let Some(proto) = self.program.functions.get(self.function as usize) else {
            return "";
        };
        self.program
            .source
            .get(proto.span.start as usize..proto.span.end as usize)
            .unwrap_or("")
    }
}

/// Two closures are equal when they are the same object, which is what `===` asks.
///
/// Structural equality would be wrong: two closures over different cells can have identical
/// protos and identical current values without being the same function.
impl PartialEq for Closure {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// An array: its elements, and the properties that are not elements.
///
/// Elements and properties are separate stores because they answer different questions. `length`
/// is derived from the elements and writing an index changes it; `index` and `input` on an
/// `exec` result, or anything a program assigns by name, are ordinary properties that do not.
/// Keeping one store would mean deciding which a numeric-looking key is on every access.
#[derive(Debug, Default)]
#[allow(clippy::mutable_key_type)]
pub struct ArrayData {
    /// The `length` the language reports, which the dense storage below does not have to reach.
    /// See [`ArrayData::length`].
    length: std::cell::Cell<usize>,
    items: RefCell<Vec<Value>>,
    /// Empty for almost every array, and allocated with it rather than on demand because an
    /// array that never gains a named property still has to answer that it has none.
    properties: RefCell<ObjectData>,
    /// Whether `length` may still change.
    ///
    /// `length` is an own data property of every array -- non-enumerable, non-configurable, and
    /// writable until a program says otherwise -- and this is the one part of it that is not
    /// derived from the elements. `Object.defineProperty(a, "length", {writable: false})` and
    /// `Object.freeze(a)` both clear it, and afterwards a write to `length`, a `push`, and an
    /// index past the end are all refused rather than silently taken.
    length_writable: std::cell::Cell<bool>,
    /// Index properties the dense storage cannot hold as it stands. Empty for almost every
    /// array, and consulted only through [`array_exotic_element`].
    elements: RefCell<Vec<ExoticElement>>,
    /// Whether this array has ever had a hole in it.
    ///
    /// A hole is the *absence* of an own property, so every read of one is a question for the
    /// prototype chain: `Array.prototype[1] = 13` really does make `[, , ,].forEach` visit
    /// index 1 with that value. The dense fast paths skip a hole outright, which is right only
    /// while nothing up the chain fills it -- so an array that has one is walked the way any
    /// other object with a `length` is, and the fast paths keep the case they are for. Set
    /// conservatively: claiming a hole an array no longer has costs speed, never correctness.
    sparse: std::cell::Cell<bool>,
    collection_index: RefCell<Option<std::collections::HashMap<CollectionKey, usize>>>,
}

/// One index property of an array that is not an ordinary element.
///
/// Either an accessor -- the dense storage holds values and nothing else -- or a data property
/// whose flags a program has moved off the ordinary writable/enumerable/configurable. A data
/// property keeps its value in the dense storage as well, so every reader that walks the
/// elements directly still sees it; an accessor leaves a hole there instead, and an array with
/// any of these stops taking the dense fast paths, so its methods walk it the way they walk any
/// other object -- through `HasProperty`, `Get` and `Set`, which is where a getter runs.
#[derive(Clone, Debug)]
struct ExoticElement {
    index: usize,
    property: Property,
    attributes: Attributes,
}

impl ArrayData {
    fn new(items: Vec<Value>) -> Rc<Self> {
        let array = Rc::new(Self {
            sparse: std::cell::Cell::new(
                items
                    .iter()
                    .any(|item| matches!(item, Value::Native(Native::Hole, _))),
            ),
            length: std::cell::Cell::new(items.len()),
            items: RefCell::new(items),
            properties: RefCell::new(ObjectData::default()),
            length_writable: std::cell::Cell::new(true),
            elements: RefCell::new(Vec::new()),
            collection_index: RefCell::new(None),
        });
        gc_census::register_array(&array);
        array
    }

    fn borrow(&self) -> std::cell::Ref<'_, Vec<Value>> {
        self.items.borrow()
    }

    fn borrow_mut(&self) -> ArrayItems<'_> {
        ArrayItems {
            items: self.items.borrow_mut(),
            length: &self.length,
        }
    }

    /// The array's `length`, which is not always the number of elements stored.
    ///
    /// `Object.defineProperty(a, "length", { value: 4294967295 })` is a legal thing to do and
    /// says nothing about what the array *contains*: the language's `length` is a count that may
    /// run far past the elements, and the dense vector holds only what is there. Deriving one
    /// from the other made that assignment an error -- and it is one the test262 harness performs
    /// on every array it verifies a property of, so it cost far more than the arrays it was
    /// about.
    fn length(&self) -> usize {
        self.length.get()
    }
}

/// A mutable borrow of an array's dense elements that keeps `length` in step.
///
/// The length is authoritative and the elements are storage, so every change to the storage has
/// to say what it did to the length. Rather than ask eighteen call sites to remember, the borrow
/// itself reports on release: for every one of them the array's length *is* the number of
/// elements, which is what makes a single rule right for all of them.
struct ArrayItems<'a> {
    items: std::cell::RefMut<'a, Vec<Value>>,
    length: &'a std::cell::Cell<usize>,
}

impl std::ops::Deref for ArrayItems<'_> {
    type Target = Vec<Value>;

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl std::ops::DerefMut for ArrayItems<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.items
    }
}

impl Drop for ArrayItems<'_> {
    fn drop(&mut self) {
        self.length.set(self.items.len());
    }
}

impl PartialEq for ArrayData {
    /// Identity: `[] === []` is false, and two references to one array are equal.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// One accessor on `RegExp.prototype`.
///
/// The specification defines every one of these as a getter rather than a data property, and the
/// difference is visible: `Object.getOwnPropertyDescriptor(RegExp.prototype, "global").get` is a
/// function with a name of its own, `RegExp.prototype.global` is `undefined` rather than a flag
/// no regexp has, and reading one off something that is not a regexp is a `TypeError`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegExpAccessor {
    Source,
    Flags,
    Global,
    IgnoreCase,
    Multiline,
    DotAll,
    Unicode,
    UnicodeSets,
    Sticky,
    HasIndices,
}

impl RegExpAccessor {
    /// The name the accessor is published under.
    const fn property_key(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Flags => "flags",
            Self::Global => "global",
            Self::IgnoreCase => "ignoreCase",
            Self::Multiline => "multiline",
            Self::DotAll => "dotAll",
            Self::Unicode => "unicode",
            Self::UnicodeSets => "unicodeSets",
            Self::Sticky => "sticky",
            Self::HasIndices => "hasIndices",
        }
    }

    /// The letter this accessor reports the presence of, or `None` for the two that are not
    /// flags at all.
    const fn letter(self) -> Option<char> {
        Some(match self {
            Self::Global => 'g',
            Self::IgnoreCase => 'i',
            Self::Multiline => 'm',
            Self::DotAll => 's',
            Self::Unicode => 'u',
            Self::UnicodeSets => 'v',
            Self::Sticky => 'y',
            Self::HasIndices => 'd',
            Self::Source | Self::Flags => return None,
        })
    }

    /// The eight single-letter accessors, in the order `flags` spells them out. The order is the
    /// specification's and it is observable: `/a/gi.flags` is "gi", never "ig".
    const FLAGS: [Self; 8] = [
        Self::HasIndices,
        Self::Global,
        Self::IgnoreCase,
        Self::Multiline,
        Self::DotAll,
        Self::Unicode,
        Self::UnicodeSets,
        Self::Sticky,
    ];

    /// Every accessor the table publishes, in the order `RegExp.prototype` lists them.
    const ALL: [Self; 10] = [
        Self::DotAll,
        Self::Flags,
        Self::Global,
        Self::HasIndices,
        Self::IgnoreCase,
        Self::Multiline,
        Self::Source,
        Self::Sticky,
        Self::Unicode,
        Self::UnicodeSets,
    ];

    /// The accessor published under one name, or `None` for a name that is not one.
    fn named(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|found| found.property_key() == key)
    }
}

/// `EscapeRegExpPattern`: the source as it would have to be written between two slashes.
///
/// Not cosmetic. The specification requires `eval("/" + re.source + "/")` to build the same
/// pattern, so the delimiter and the four line terminators have to be escaped -- and a backslash
/// takes the character after it along, or an already-escaped slash would be escaped twice.
fn escaped_source(source: &str) -> String {
    if source.is_empty() {
        // The empty pattern has no spelling between slashes -- `//` opens a comment -- so the
        // specification substitutes the one group that matches the same thing.
        return "(?:)".to_owned();
    }
    let mut escaped = String::with_capacity(source.len());
    let mut in_class = false;
    let mut characters = source.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                escaped.push('\\');
                if let Some(taken) = characters.next() {
                    escaped.push(taken);
                }
            }
            '[' => {
                in_class = true;
                escaped.push('[');
            }
            ']' => {
                in_class = false;
                escaped.push(']');
            }
            // Inside a character class the delimiter is an ordinary character, and escaping it
            // there would change what the pattern says it matches.
            '/' if !in_class => escaped.push_str("\\/"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\u{2028}' => escaped.push_str("\\u2028"),
            '\u{2029}' => escaped.push_str("\\u2029"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn regexp_string_units(regexp: &RegExpData) -> StringUnits {
    let source = escaped_source_units(&regexp.source);
    let mut units = Vec::with_capacity(source.unit_len() + regexp.flags.len() + 2);
    units.extend("/".encode_utf16());
    units.extend(source.units());
    units.extend("/".encode_utf16());
    units.extend(regexp.flags.encode_utf16());
    StringUnits::from_units(&units)
}

fn escaped_source_units(source: &StringUnits) -> StringUnits {
    if let Ok(scalar) = source.to_scalar_string() {
        return StringUnits::from(escaped_source(&scalar));
    }
    // `source` hands back the pattern text, and a lone surrogate is the unit it is:
    // only scalar text goes through the escaper, which has no spelling for one.
    let mut units = Vec::with_capacity(source.unit_len());
    for unit in source.units() {
        if (0xd800..=0xdfff).contains(&unit) {
            units.push(unit);
        } else if let Some(character) = char::from_u32(u32::from(unit)) {
            units.extend(escaped_source(&character.to_string()).encode_utf16());
        }
    }
    StringUnits::from_units(&units)
}

/// A compiled regular expression, its source, and the cursor `g` and `y` advance.
///
/// The engine is `regress`, which targets ECMAScript semantics rather than the linear-time
/// subset: backreferences, lookaround and sticky matching are all things a JavaScript program
/// expects and a linear-time engine cannot offer. It also matches over UTF-16, so an index it
/// reports is already in the units the language counts in and needs no conversion.
#[derive(Debug)]
pub struct RegExpData {
    /// The pattern as written, which `source` hands back to the program.
    source: StringUnits,
    flags: String,
    /// Immutable compiled code can be shared across realms. The regexp object,
    /// its properties and its cursor remain fresh for every construction.
    compiled: Arc<regress::Regex>,
    global: bool,
    sticky: bool,
    /// The `d` flag. Read once here rather than off `flags` at every match, and kept beside the
    /// other two for the same reason they are: a flag the matcher consults is not a string search.
    has_indices: bool,
    /// The properties a program put on the regexp by name, and `lastIndex` among them.
    ///
    /// It was a `Cell` beside this store, so that the search and a read of `r.lastIndex` could
    /// not disagree. One store cannot disagree with itself either, and the slot could not carry
    /// what the language says `lastIndex` is: an own property with attributes. It had no
    /// descriptor, `defineProperty` did not reach it, and a strict write to a frozen one
    /// succeeded -- where the specification writes it with `Set(..., true)` and throws.
    properties: RefCell<ObjectData>,
}

impl PartialEq for RegExpData {
    /// Identity, like every other object: two literals with the same source are two regexps.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A `Date`: the instant it names, and the ordinary properties a program hung beside it.
///
/// The store is not decoration. A date is an ordinary object in every respect but the instant --
/// `d.constructor = C` and `class D extends Date {}` are both property writes -- and while there
/// was nowhere to put one, the first threw "cannot set a property on a date" and the second had
/// nowhere to record the prototype that makes `new D(0) instanceof D` true.
#[derive(Debug)]
pub struct DateData {
    /// Milliseconds since the epoch. A `Cell` because `d.setFullYear(y)` moves *this* date rather
    /// than answering a new one.
    time: std::cell::Cell<f64>,
    properties: RefCell<ObjectData>,
}

impl PartialEq for DateData {
    /// Identity, like every other object: two dates built from the same instant are two dates.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// One of the eight `Temporal` types, and the ordinary properties a program hung beside it.
///
/// The store is here for the reason `Date`'s is, and the failure it fixes is the same one: a
/// `Temporal` value is an ordinary object in every respect but the calendar state it carries, so
/// `d.constructor = C` and `class D extends Temporal.Duration {}` are both ordinary property
/// writes. While there was nowhere to put one, the first threw "cannot set a property on a
/// Temporal.Duration" -- which is this engine reporting it had nowhere to write, not the language
/// forbidding the write -- and `Object.defineProperty(date, "year", ...)` answered that it had
/// recorded a definition it had in fact dropped.
#[derive(Debug)]
pub struct TemporalData {
    /// The calendar value, which nothing here ever writes: every `Temporal` type is immutable and
    /// an operation on one answers a new value rather than moving this one.
    value: kiln_temporal::TemporalValue,
    properties: RefCell<ObjectData>,
}

impl PartialEq for TemporalData {
    /// Identity, like every other object: two values built from the same instant are two objects.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl TemporalData {
    /// The calendar value behind the object, for the operations that are the engine's.
    pub(crate) fn value(&self) -> &kiln_temporal::TemporalValue {
        &self.value
    }

    /// Which of the eight types this is.
    pub(crate) fn kind(&self) -> kiln_temporal::Kind {
        self.value.kind()
    }
}

/// A date at `time`, with an empty property store.
pub(crate) fn new_date(time: f64) -> Value {
    Value::Date(Rc::new(DateData {
        time: std::cell::Cell::new(time),
        properties: RefCell::new(ObjectData::default()),
    }))
}

/// Limits that make execution reproducible and prevent an accidental infinite loop from hanging
/// the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionLimits {
    /// How deep a built-in may call back into JavaScript before the machine refuses.
    ///
    /// Separate from `max_call_depth` because it bounds a different resource: this one is the
    /// Rust stack, which cannot be grown and whose exhaustion is not catchable.
    pub max_native_depth: usize,
    pub max_steps: u64,
    /// Frames allowed on the call stack.
    ///
    /// A separate bound from `max_steps` because unbounded recursion exhausts the host stack
    /// long before it exhausts a step budget, and the failure mode is a process abort rather
    /// than a diagnostic. Bounding frames turns it into an error the caller can report.
    pub max_call_depth: usize,
}

impl ExecutionLimits {
    /// `max_steps` for an execution nothing is waiting on.
    ///
    /// A step budget is a promise to whoever is queued behind this program -- a corpus runner, a
    /// resident daemon -- that a loop which never ends will be reported rather than endured. A
    /// person running their own program has made no such promise, and node makes none either.
    /// The counter still runs, so `RunSummary::instructions` still reports what the program cost;
    /// it simply has nothing to exceed.
    pub const UNMETERED_STEPS: u64 = u64::MAX;

    /// The default limits with a different step budget.
    #[must_use]
    pub const fn with_max_steps(self, max_steps: u64) -> Self {
        Self { max_steps, ..self }
    }
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            // Roughly what node allows. A JavaScript frame is a `Frame` in a vector rather than
            // a Rust stack frame, so ordinary recursion costs heap and not stack; 512 refused
            // programs node runs.
            max_call_depth: 10_000,
            // The interpreter *does* recurse in Rust when a built-in calls back into JavaScript
            // -- `map` running a callback that calls `map` -- and that recursion is bounded by
            // the real stack rather than by a vector. A nested array callback at 125 levels
            // overflowed a 16 MiB program thread before the previous limit of 192 fired.
            // Leave room for different native paths and Rust frame sizes while reporting a
            // catchable RangeError instead of aborting the process.
            max_native_depth: 64,
        }
    }
}

/// Source text handed to a run-time compilation, with everything the compiler cannot see.
///
/// `outer` is empty for an indirect eval and for `Function`, both of which evaluate in global
/// scope. A direct eval fills it with the caller's visible bindings, in the slot order
/// [`kiln_ir::compile_eval`] promises to bind them to.
pub struct EvalSource<'a> {
    /// Scalar UTF-8 view for compatibility with existing compiler callbacks. When
    /// [`Self::source_units`] is `Some`, this may contain replacement characters; compilers must
    /// treat the exact UTF-16 units as authoritative source.
    pub source: &'a str,
    /// Exact JavaScript UTF-16 source when the transport string cannot represent it (for example,
    /// a source body containing a lone surrogate). Compilers should prefer this when present.
    pub source_units: Option<&'a StringUnits>,
    /// Stable module/call-site identity for private scopes created by this dynamic compilation.
    pub module_id: &'a str,
    pub outer: &'a [kiln_ir::EvalOuter],
    /// Names among `outer` that hold the caller's active `with` object records.
    pub with_objects: &'a [String],
    pub strict: bool,
    /// Whether this eval's `var` and function declarations belong to the global environment
    /// record, and so become properties of the global object rather than slots of its own frame.
    pub global_var_scope: bool,
    /// Whether this source is a Script rather than eval code: its `var`s reach the global object
    /// even in strict mode, and the bindings it makes there are not configurable.
    pub script: bool,
    /// The name, among `outer`, of the caller's home-object binding -- `None` where the language
    /// does not allow `super.x` to be written at this call.
    pub home: Option<&'a str>,
    /// What `new.target` means here, including whether it may be written at all.
    pub new_target: kiln_ir::EvalNewTarget,
    /// The private names the calling code can see, outermost class first.
    pub private_scopes: &'a [kiln_ir::PrivateScope],
    /// Names a `var` in this source may not create, from [`kiln_ir::EvalSite::blocked_var_names`].
    pub blocked_var_names: &'a [String],
}

/// Why a run-time compilation produced no program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalCompileError {
    /// The source is not a program. A `SyntaxError` the calling code can catch, which is what
    /// `assert.throws(SyntaxError, () => eval(bad))` is written to see.
    Syntax(String),
    /// A program this compiler does not implement yet. Kept apart from a syntax error on
    /// purpose: reporting `SyntaxError` for a gap tells the program its source was invalid, and
    /// a program that believes that will take a different branch rather than fail.
    Unsupported(String),
}

/// How a host turns source text into a program while a program is already running.
///
/// `eval` and `Function` are the only two things in the language that need a compiler at run
/// time, and the interpreter cannot own one: ADR 0007 keeps it downstream of the bytecode and
/// clear of the frontend, so an `eval` that parsed its own source would pull Oxc into every build
/// of the VM. The frontend is injected instead -- the same shape ADR 0006 gives `kiln-db`, one
/// function pointer over owned data -- and a host that installs none simply has no `eval`.
pub type SourceCompiler = fn(&EvalSource<'_>) -> Result<(Program, Strictness), EvalCompileError>;

/// The program every realm an execution makes runs once its global object exists.
///
/// The built-ins ADR 0060 gives to shared TypeScript (`kiln-builtins`: `Date.prototype`,
/// `%TypedArray%.prototype`, `ArrayBuffer.prototype.transfer`, `structuredClone`, ...) are
/// installed by running that TypeScript, so a realm that skipped it would answer with whatever
/// Rust duplicate happened to remain -- or with nothing, once the duplicate is deleted. That is
/// exactly how Test262 lost `transfer`: its script path built realms that never ran it.
///
/// Compiled by the embedder and handed over as a function pointer, the shape [`SourceCompiler`]
/// has and for the same reason: the interpreter owns no frontend. The function answers the same
/// immutable program every time -- an embedder caches it, so it is compiled once, not once per
/// realm -- and the machine runs it in every realm it initialises: the entry realm, each
/// `$262.createRealm` realm and each `ShadowRealm`. Only code is shared; every object the
/// bootstrap builds is built afresh in the realm that runs it (ADR 0038, ADR 0071).
pub type RealmBootstrap = fn() -> Result<Rc<Program>, String>;

/// The module path a realm bootstrap runs under: what its stack frames and its compile
/// diagnostics name. Public so an embedder compiles it under the same name the machine runs it.
pub const REALM_BOOTSTRAP_PATH: &str = "@kiln/realm-bootstrap.ts";

/// Language-engine capabilities selected explicitly for one execution realm.
///
/// Environmental values remain [`RealmCapabilities`] owned by `kiln-runtime`. This companion
/// keeps compiler/linker implementation types out of that scheduler crate while making their
/// authority just as explicit. The default realm cannot compile source at run time.
#[derive(Clone, Default)]
pub struct ExecutionCapabilities {
    source_compiler: Option<SourceCompiler>,
    realm_bootstrap: Option<RealmBootstrap>,
    module_loader: Option<Arc<dyn ModuleLoaderService + Send + Sync>>,
    disallow_string_code_generation: bool,
    host_realms: bool,
    host_agents: Option<&'static str>,
    /// The command line this execution's `process.argv` reports, exactly as the host spelled it.
    ///
    /// Empty by default, and empty is not the same as `[node, script]` with the names left off:
    /// an execution nobody launched from a command line has no command line, and inventing one
    /// would put a made-up executable path in front of a program that asked where it came from.
    /// A host that has one states it, which is the same bargain as every other entry here.
    argv: Vec<String>,
    /// Runtime flags this execution reports through `process.execArgv`.
    exec_argv: Vec<String>,
    /// How this execution starts a `node:worker_threads` worker's realm, when it can.
    worker_host: Option<Arc<dyn WorkerHost>>,
}

/// What starts a worker's realm (`node:worker_threads`, realms issue 06): the host that has the
/// program's module graph. The interpreter owns no linker (see [`PlannedModule`]), so it cannot
/// run a module it was not handed; the host can, on the worker's own thread.
pub trait WorkerHost: Send + Sync {
    /// Whether `path` is a module of this program a worker can run.
    fn has_module(&self, path: &str) -> bool;
    /// Run the worker's program to its end on the calling thread, which is the worker's; the
    /// exit code its parent's `exit` event reports.
    fn run(&self, worker: Arc<kiln_runtime::workers::WorkerContext>) -> i32;
}

impl ExecutionCapabilities {
    /// Disallow `eval` and function constructors when this execution is launched with Node's
    /// `--disallow-code-generation-from-strings` policy.
    pub const fn with_string_code_generation(mut self, allowed: bool) -> Self {
        self.disallow_string_code_generation = !allowed;
        self
    }

    pub const fn string_code_generation_allowed(&self) -> bool {
        !self.disallow_string_code_generation
    }

    pub const fn with_source_compiler(mut self, compiler: SourceCompiler) -> Self {
        self.source_compiler = Some(compiler);
        self
    }

    pub const fn source_compiler(&self) -> Option<SourceCompiler> {
        self.source_compiler
    }

    /// Run `bootstrap`'s program in every realm this execution initialises. See
    /// [`RealmBootstrap`]. Without one a realm has only the machine's own intrinsics, which is
    /// what an embedder testing the bare machine asks for.
    pub const fn with_realm_bootstrap(mut self, bootstrap: RealmBootstrap) -> Self {
        self.realm_bootstrap = Some(bootstrap);
        self
    }

    pub const fn realm_bootstrap(&self) -> Option<RealmBootstrap> {
        self.realm_bootstrap
    }

    pub fn with_module_loader_service(
        mut self,
        loader: Arc<dyn ModuleLoaderService + Send + Sync>,
    ) -> Self {
        self.module_loader = Some(loader);
        self
    }

    pub fn module_loader(&self) -> Option<Arc<dyn ModuleLoaderService + Send + Sync>> {
        self.module_loader.clone()
    }

    /// Let this execution make further realms, and publish the hook that does it.
    ///
    /// Off by default and named by the host rather than inferred, for ADR 0031's reason: a realm
    /// takes what it can do from the host explicitly, so a program cannot acquire a second realm
    /// because it happened to be run by a runner that had one. Test262's `$262.createRealm` is
    /// what asks for it.
    pub const fn with_host_realms(mut self) -> Self {
        self.host_realms = true;
        self
    }

    pub const fn host_realms(&self) -> bool {
        self.host_realms
    }

    /// Let this execution start a second agent, and say what a new agent's realm is told about
    /// its host before it runs the program's source.
    ///
    /// The prelude is the *host's* text, not the engine's, for the same reason `$262` itself is
    /// assembled by the runner: a new agent needs a `$262.agent` to call `receiveBroadcast` and
    /// `report` on, and what a host can do is the host's statement to make. Passing it here also
    /// keeps the interpreter from having a Test262-shaped string in it.
    ///
    /// Off by default, like [`Self::with_host_realms`] and for ADR 0031's reason: a program does
    /// not acquire a second thread because it happened to be run by a runner that has one.
    pub const fn with_host_agents(mut self, prelude: &'static str) -> Self {
        self.host_agents = Some(prelude);
        self
    }

    pub const fn host_agents(&self) -> Option<&'static str> {
        self.host_agents
    }

    /// State the command line this execution runs under, as `process.argv` should report it.
    ///
    /// The host supplies the whole vector, including the two leading entries, because only the
    /// host knows what they are: which executable is running and what it was pointed at.
    pub fn with_argv(mut self, argv: Vec<String>) -> Self {
        self.argv = argv;
        self
    }

    /// State the startup flags this execution reports through `process.execArgv`.
    pub fn with_exec_argv(mut self, exec_argv: Vec<String>) -> Self {
        self.exec_argv = exec_argv;
        self
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn exec_argv(&self) -> &[String] {
        &self.exec_argv
    }

    /// Let this execution start workers, through `host`.
    pub fn with_worker_host(mut self, host: Arc<dyn WorkerHost>) -> Self {
        self.worker_host = Some(host);
        self
    }

    pub fn worker_host(&self) -> Option<Arc<dyn WorkerHost>> {
        self.worker_host.clone()
    }
}

/// One module a dynamic import may have to evaluate, compiled and linked by the host.
///
/// The interpreter deliberately owns no linker for the same reason it owns no parser: resolving
/// a specifier is filesystem policy and reading a star re-export is graph work, and neither
/// belongs in a machine that runs bytecode. What arrives here is already decided -- and linked
/// by *name* against the realm's [`ModuleMap`], so a module some other importer (the static
/// graph, or another `import()`) already evaluated is linked to rather than run again.
pub struct PlannedModule {
    /// The module's canonical key -- its path -- which is its key in the realm's module map.
    pub key: String,
    /// How the module is evaluated if the map does not already hold it.
    pub body: PlannedBody,
    /// Module-scope slots to fill before the body runs, and where each value comes from.
    pub bindings: Vec<(u32, ModuleBinding)>,
    /// Names this module exports that are not its own bindings (`export *`, `export * as ns`),
    /// and where each one comes from.
    pub reexports: Vec<(String, ModuleBinding)>,
    /// Canonical static dependencies, including side-effect-only imports. The interpreter uses
    /// these edges to preserve module evaluation ordering when one body suspends at top-level
    /// await; bindings alone omit side-effect-only edges.
    pub dependencies: Vec<String>,
    /// Strongly-connected component in the host's dependency graph. Members of one component
    /// begin evaluation together before an external importer waits for their completion.
    pub evaluation_component: u32,
}

/// How a planned module is evaluated.
pub enum PlannedBody {
    /// A body to run. Its `Return` yields an array holding the module's own exports, in the
    /// order `exports` names them.
    ///
    /// The epilogue that builds that array is the host's work, because only the host has a
    /// compiler: nothing here can look inside a module and decide which slots it exports.
    Source {
        program: Rc<Program>,
        exports: Vec<String>,
        /// An engine builtin module: while its top-level initialization runs, the realm's
        /// engine-private object is readable as the global `__kilnPrivate` (see
        /// [`open_private_window`]). No program module ever sees it.
        engine_private: bool,
    },
    /// Exports the host computed without running anything (a file asset's path).
    Exports(Vec<(String, Value)>),
    /// A module the host's CommonJS loader evaluates (CommonJS, JSON), through the same cache
    /// `require` uses, so `import()` and `require` of it share one `module.exports`.
    Host,
}

/// Where one value a planned module imports comes from.
#[derive(Clone, Debug)]
pub enum ModuleBinding {
    /// The export `name` of the module whose key is `module`.
    Export { module: String, name: String },
    /// `import * as ns`: the namespace object of the module whose key is `module`.
    Namespace { module: String },
    /// A value the host already holds.
    Ready(Value),
}

/// Everything one `import(specifier)` needs, in the order it has to happen.
pub struct ModulePlan {
    /// The key of the module that was asked for, which is one of `modules`' keys.
    pub key: String,
    /// Dependencies first, with the requested module last. A module the realm's map already
    /// holds is not evaluated again.
    pub modules: Vec<PlannedModule>,
}

/// Why a host produced no plan for a specifier.
pub enum ModuleLoadError {
    /// The specifier names nothing, or names something the graph will not link. The promise
    /// rejects, which is what a host is supposed to do with a specifier it cannot resolve.
    NotFound(String),
    /// A requested module is not a module. A `SyntaxError`, raised at link time.
    Syntax(String),
    /// A module this compiler cannot build yet, or an unsupported dynamic import. Rejects the
    /// returned promise with a `TypeError` per ECMA-262 §13.3.10.1 (`HostLoadImportedModule`).
    Unsupported(String),
}

/// How a host turns one `import(specifier)` into modules the machine can run.
///
/// The same shape [`SourceCompiler`] has, and for the same reason (ADR 0007, ADR 0030): the
/// interpreter is downstream of the bytecode and has no filesystem, no parser and no linker.
/// A host that installs none simply has no `import()`, and says so rather than answering.
///
/// The complete context of one dynamic import request. The service is scoped to an execution
/// realm, so a loader can retain graph ownership without ambient process state.
#[derive(Clone)]
pub struct ModuleLoadRequest {
    pub importer: Option<String>,
    pub specifier: String,
    pub attributes: Option<ImportAttributes>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportAttributes {
    pub type_: Option<String>,
}

pub trait ModuleLoaderService: Send + Sync {
    fn load(&self, request: ModuleLoadRequest) -> Result<ModulePlan, ModuleLoadError>;
}

/// Whether a program reads or writes module scope through the frame stack.
///
/// True of every ordinary program with a nested function over a module binding, and false of
/// every eval program, which boxes its top-level bindings instead. That asymmetry is the point:
/// a `LoadGlobal` reaches the frame `global_base` names, and for an eval that frame belongs to a
/// program with entirely different slots.
fn reads_module_scope(program: &Program) -> bool {
    fn reaches(code: &[Instruction]) -> bool {
        code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadGlobal(_) | Instruction::StoreGlobal(_)
            )
        })
    }
    reaches(&program.code) || program.functions.iter().any(|proto| reaches(&proto.code))
}

/// Whether source text opens with a `"use strict"` directive of its own.
///
/// Read from the text because a directive prologue does not survive lowering: the frontend keeps
/// statements, and a directive is not one. Only the leading run of string-literal statements is
/// examined, which is what a prologue is, and an escaped spelling -- `"use\u0020strict"` -- is
/// deliberately not a directive and is not recognised here either, exactly as the specification
/// requires.
fn source_is_strict(source: &str) -> bool {
    let mut rest = source;
    loop {
        rest = skip_trivia(rest);
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            return false;
        };
        let body = &rest[quote.len_utf8()..];
        let Some(end) = body.find([quote, '\\', '\n', '\r']) else {
            return false;
        };
        // A backslash means the literal is escaped, so its characters are not the ones written
        // and it cannot be the directive whatever it spells.
        if body.as_bytes()[end] != quote as u8 {
            return false;
        }
        if &body[..end] == "use strict" {
            return true;
        }
        rest = &body[end + 1..];
        rest = skip_trivia(rest);
        rest = rest.strip_prefix(';').unwrap_or(rest);
    }
}

/// Source with leading whitespace and comments removed.
fn skip_trivia(source: &str) -> &str {
    let mut rest = source;
    loop {
        let trimmed = rest.trim_start();
        if let Some(after) = trimmed.strip_prefix("//") {
            rest = after.find(['\n', '\r']).map_or("", |end| &after[end..]);
            continue;
        }
        if let Some(after) = trimmed.strip_prefix("/*") {
            let Some(end) = after.find("*/") else {
                return "";
            };
            rest = &after[end + 2..];
            continue;
        }
        return trimmed;
    }
}

/// The successful result of one VM invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionResult {
    pub value: Value,
    pub steps: u64,
    /// The exit status requested through `process.exitCode`, if any.
    pub process_exit_code: Option<i32>,
}

/// One value published by a completed module evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleExportValue {
    pub name: String,
    pub value: Value,
}

/// An import a module makes from its own specifier.
///
/// Every other import is a value the linker already has, because the module it came from ran
/// first. A self-import has no such value and never will: the binding it names is created by the
/// very body that is about to run. So the linker hands over the *place* instead of the value,
/// and the machine ties the two together as the body creates them.
#[derive(Clone, Debug, PartialEq)]
pub enum ModuleSelfImport {
    /// `import { x } from` this module's own specifier. The import slot is made an alias of the
    /// export slot: from the moment the body creates the cell, both slots hold that one cell,
    /// which is exactly what a live binding is.
    Binding { import_slot: u32, export_slot: u32 },
    /// `import * as ns from` this module's own specifier. The object exists before anything has
    /// been assigned, so it is built over the slots and caught up from them rather than
    /// snapshotted -- a snapshot taken here would be a namespace of undefineds.
    Namespace {
        import_slot: u32,
        /// Every visible export, in the order a namespace holds them.
        exports: Vec<(String, ModuleSelfExport)>,
    },
}

/// Where one name of a self-imported namespace gets its value.
#[derive(Clone, Debug, PartialEq)]
pub enum ModuleSelfExport {
    /// A binding of this module, by the local slot that will hold its cell.
    Slot(u32),
    /// A name re-exported from a module that has already finished, so its value is known.
    Settled(Value),
}

/// A namespace a module imported from itself, beside where each of its names comes from.
type SelfNamespace = (Rc<RefCell<ObjectData>>, Vec<(String, ModuleSelfExport)>);

/// A completed module invocation, including the values its local export slots hold.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleExecutionResult {
    pub execution: ExecutionResult,
    pub exports: Vec<ModuleExportValue>,
    /// The final value of the CommonJS `module.exports` object when the module wrapper supplied
    /// one. Read through the VM so an accessor descriptor is observed like an ordinary require.
    pub commonjs_exports: Option<Value>,
}

/// Realm-owned state shared by every module body in one graph execution.
///
/// Module frames remain short-lived, but Node resources and language jobs do not: a dependency
/// can create a server whose callback runs while the entry module is active. This session is the
/// explicit ownership seam that prevents those resources from becoming ambient globals or being
/// stranded in the VM that happened to evaluate their declaring module.
#[derive(Clone)]
pub struct ModuleRealmSession {
    shared: Rc<RefCell<ModuleRealmShared>>,
    /// Whether the Node execution adapter (host I/O, HTTP) is installed in this realm. A graph
    /// shares one realm in every mode; the adapter is what `--node` adds, and a plain run keeps
    /// refusing `node:fs` and the like as it always did.
    adapter: bool,
}

impl ModuleRealmSession {
    /// A shared realm with no host adapter: one global object for the graph, the virtual clock,
    /// and every host-reaching operation refusing as a plain run's did.
    pub fn without_adapter() -> Self {
        Self {
            adapter: false,
            ..Self::default()
        }
    }

    /// Open the engine-private window for one engine builtin module's top-level initialization.
    /// The module evaluator calls `end_engine_module` on both normal and throwing exits; windows
    /// nest, as a lazily loaded builtin may initialize inside another's window.
    pub fn begin_engine_module(&mut self) {
        let mut shared = self.shared.borrow_mut();
        let window = match shared.global_objects.first().cloned().flatten() {
            Some(Value::Object(global)) => open_private_window(&mut global.borrow_mut()),
            // The realm is built lazily inside this module's own run, after the window opens;
            // `realize_realm` finishes opening it once the realm's bootstrap has returned.
            _ => PrivateWindow::Absent,
        };
        shared.private_windows.push(window);
    }

    /// The realm's module map, which the graph executor evaluates its modules into.
    pub fn module_map(&self) -> SharedModuleMap {
        Rc::clone(&self.shared.borrow().module_map)
    }

    pub fn end_engine_module(&mut self) {
        let mut shared = self.shared.borrow_mut();
        let window = shared.private_windows.pop();
        if let (Some(window), Some(Value::Object(global))) =
            (window, shared.global_objects.first().cloned().flatten())
        {
            close_private_window(&mut global.borrow_mut(), window);
        }
    }
}

/// The internal slot of a realm's global object that holds its engine-private object. Not a
/// property: reflection never lists it and a program's own string key cannot reach it.
pub(crate) const ENGINE_PRIVATE_SLOT: &str = "@@kiln:engine-private";
/// The one global name that exposes the engine-private object, and only inside a window.
const ENGINE_PRIVATE_GLOBAL: &str = "__kilnPrivate";
const ENGINE_PRIVATE_FOR_TESTS_ENV: &str = "KILN_ENGINE_PRIVATE_FOR_TESTS";

/// The realm's engine-private object: the one channel between the engine, the realm bootstrap and
/// the builtin modules (ADR 0068). It carries the engine's own operations -- present only when
/// kiln's extensions are -- and whatever one builtin hands another (the message-port internals,
/// the structured clone's marks). A program can never read it: the bootstrap captures it from
/// `globalThis` and deletes the name, and each builtin module sees it only while its top-level
/// initialization runs, in a window the evaluator opens and closes around that code alone.
pub(crate) fn new_engine_private(realm: RealmId) -> Value {
    let mut private = ObjectData::default();
    let extensions = kiln_builtin_surface::kiln_extensions_installed();
    private.set_non_enumerable("extensions", Value::Boolean(extensions));
    // A developer switch for the conformance probes that drive the actor protocol directly: the
    // bootstrap then leaves the global name in place. Off unless the process is started with it.
    let exposed = std::env::var(ENGINE_PRIVATE_FOR_TESTS_ENV).is_ok_and(|value| value == "1");
    private.set_non_enumerable("exposed", Value::Boolean(exposed));
    if extensions {
        for (name, native) in [
            ("share", Native::ShareRaw),
            ("sharedIs", Native::SharedIsRaw),
        ] {
            private.set_non_enumerable(name, Value::Native(native, realm));
        }
    }
    // Node's child_process adapter shares this private asynchronous host capability with
    // kiln:process. Keep it available when public Kiln extensions are disabled as well.
    private.set_non_enumerable(
        "hostProcessStart",
        Value::Native(Native::ProcessStartRaw, realm),
    );
    private.set_non_enumerable(
        "hostProcessCancel",
        Value::Native(Native::ProcessCancelRaw, realm),
    );
    private.set_non_enumerable(
        "processExitCodeGet",
        Value::Native(Native::ProcessExitCodeGet, realm),
    );
    private.set_non_enumerable(
        "processExitCodeSet",
        Value::Native(Native::ProcessExitCodeSet, realm),
    );
    private.set_non_enumerable("gcRequest", Value::Native(Native::GcRequest, realm));
    Value::Object(tracked_object(private))
}

/// What closing a window has to undo.
#[derive(Clone, Debug)]
pub(crate) enum PrivateWindow {
    /// No window was opened: the realm has no engine-private object yet.
    Absent,
    /// The name was already reading the engine-private object (an enclosing window).
    Nested,
    /// This window set the name, over `prior` when a program had defined one.
    Open(Option<Slot>),
    /// The activation is suspended. The global name has been restored and will be opened again
    /// when this module body resumes.
    Parked,
}

/// Make the realm's engine-private object readable as `globalThis.__kilnPrivate`.
///
/// Only this one property is added and removed again: nothing else a builtin defines on the
/// global object during its initialization is touched, and a program's own property of that name
/// is put back as it was.
pub(crate) fn open_private_window(global: &mut ObjectData) -> PrivateWindow {
    let Some(Value::Object(private)) = global.get_own(ENGINE_PRIVATE_SLOT).cloned() else {
        return PrivateWindow::Absent;
    };
    let existing = global
        .properties
        .iter()
        .position(|slot| PropertyKeyRef::Utf8(ENGINE_PRIVATE_GLOBAL).matches_slot(slot));
    if let Some(index) = existing
        && matches!(
            &global.properties[index].property,
            Property::Value(Value::Object(current)) if Rc::ptr_eq(current, &private)
        )
    {
        return PrivateWindow::Nested;
    }
    let prior = existing.map(|index| global.properties[index].clone());
    global.set_non_enumerable(ENGINE_PRIVATE_GLOBAL, Value::Object(private));
    PrivateWindow::Open(prior)
}

pub(crate) fn close_private_window(global: &mut ObjectData, window: PrivateWindow) {
    if let PrivateWindow::Open(prior) = window {
        global.remove(ENGINE_PRIVATE_GLOBAL);
        if let Some(slot) = prior {
            global.properties.push(slot);
        }
    }
}

pub(crate) fn park_private_window(global: &mut ObjectData, window: &mut Option<PrivateWindow>) {
    let Some(current) = window.take() else {
        return;
    };
    match current {
        PrivateWindow::Open(prior) => {
            close_private_window(global, PrivateWindow::Open(prior));
            *window = Some(PrivateWindow::Parked);
        }
        PrivateWindow::Nested => *window = Some(PrivateWindow::Parked),
        other => *window = Some(other),
    }
}

pub(crate) fn resume_private_window(global: &mut ObjectData, window: &mut Option<PrivateWindow>) {
    if matches!(window, Some(PrivateWindow::Parked)) {
        *window = Some(open_private_window(global));
    }
}

// A pointer identifies the compiled call site only while its allocation cannot be reused.
// Keep a weak owner beside the value: eval may release its program's code, but the Rc allocation
// remains reserved until this cache entry dies, preventing a later eval from inheriting its site.
type TemplateObjectCache =
    std::collections::HashMap<(RealmId, usize, u32), (std::rc::Weak<Program>, Value)>;

struct ModuleRealmShared {
    realm: Realm<Scheduled, Timer>,
    global_objects: Vec<Option<Value>>,
    regexp_legacy_captures: Vec<[StringUnits; 9]>,
    /// One entry per engine module initializing right now, innermost last.
    private_windows: Vec<PrivateWindow>,
    pending_host: Vec<PendingHostRequest>,
    pending_node: Vec<PendingNodeRequest>,
    rejections: Rejections,
    node_servers: Vec<Rc<RefCell<NodeServerState>>>,
    /// Strong roots for server EventEmitter objects while their listeners are live. Kept apart
    /// from NodeServerState so ObjectData -> state does not form an Rc cycle; dropping the shared
    /// realm also releases every active root on abort/teardown.
    active_node_server_objects: std::collections::HashMap<usize, Value>,
    active_node_socket_objects: std::collections::HashMap<usize, Value>,
    http_client_requests: std::collections::BTreeMap<u64, Rc<RefCell<HttpClientRequestState>>>,
    next_http_client_request_id: u64,
    symbol_serial: u64,
    symbol_registry: Vec<(StringUnits, Value)>,
    symbols: Vec<Rc<UniqueSymbol>>,
    template_objects: TemplateObjectCache,
    /// Immutable eval bindings travel with closures across module VMs in this graph realm.
    readonly_cells: vm::SharedReadonlyCellRegistry,
    intrinsics: Rc<Intrinsics>,
    isolated_graphs: Rc<RefCell<Vec<(Weak<RefCell<ObjectData>>, Value)>>>,
    actor_realm_id: u64,
    actor_graph_requests: Rc<RefCell<std::collections::HashMap<u64, u8>>>,
    /// The realm's module map: one record per module, shared by the static graph executor and
    /// every machine that runs `import()` in this realm.
    module_map: SharedModuleMap,
}

impl Default for ModuleRealmSession {
    fn default() -> Self {
        Self {
            adapter: true,
            shared: Rc::new(RefCell::new(ModuleRealmShared {
                realm: Realm::new(),
                global_objects: vec![None],
                regexp_legacy_captures: vec![std::array::from_fn(|_| StringUnits::default())],
                private_windows: Vec::new(),
                pending_host: Vec::new(),
                pending_node: Vec::new(),
                rejections: Rejections::default(),
                node_servers: Vec::new(),
                active_node_server_objects: std::collections::HashMap::new(),
                active_node_socket_objects: std::collections::HashMap::new(),
                http_client_requests: std::collections::BTreeMap::new(),
                next_http_client_request_id: 1,
                symbol_serial: 0,
                symbol_registry: Vec::new(),
                symbols: Vec::new(),
                template_objects: std::collections::HashMap::new(),
                readonly_cells: Rc::new(RefCell::new(vm::ReadonlyCellRegistry::default())),
                intrinsics: Rc::new(Intrinsics::default()),
                isolated_graphs: Rc::new(RefCell::new(Vec::new())),
                actor_realm_id: kiln_runtime::workers::actor_request_id(),
                actor_graph_requests: Rc::new(RefCell::new(std::collections::HashMap::new())),
                module_map: SharedModuleMap::default(),
            })),
        }
    }
}

mod execution_api;
use execution_api::*;
pub use execution_api::{
    execute, execute_in_runtime_with_strictness, execute_module, execute_module_in_runtime,
    execute_module_in_runtime_with_loader, execute_node, execute_node_in_runtime_with_strictness,
    execute_node_module_in_runtime, execute_node_module_in_session,
    execute_node_module_in_session_with_loader, execute_node_with_clock_and_strictness,
    execute_node_with_strictness, execute_reclaiming,
    execute_reclaiming_in_runtime_with_strictness, execute_reclaiming_with_strictness,
    execute_with_strictness, namespace_entries, namespace_object, ordinary_object,
    own_data_property, parse_json_module, read_through_cell, worker_error_tokens,
};
#[path = "vm/state.rs"]
mod vm_state;
use vm_state::*;
pub use vm_state::{ElementFunction, PromiseData, PromiseState};
#[path = "vm/agent.rs"]
mod vm_agent;
#[path = "vm/arguments.rs"]
mod vm_arguments;
#[path = "vm/array_grouping.rs"]
mod vm_array_grouping;
#[path = "vm/array_helpers.rs"]
mod vm_array_helpers;
#[path = "vm/array_like_ops.rs"]
mod vm_array_like;
#[path = "vm/async_context.rs"]
mod vm_async_context;
#[path = "vm/async_generators.rs"]
mod vm_async_generators;
#[path = "vm/atomics.rs"]
mod vm_atomics;
#[path = "vm/call_eval.rs"]
mod vm_call_eval;
#[path = "vm/call_dispatch.rs"]
mod vm_calls;
#[path = "vm/closures.rs"]
mod vm_closures;
#[path = "vm/collection_construction.rs"]
mod vm_collection_construction;
#[path = "vm/collections.rs"]
mod vm_collections;
#[path = "vm/console.rs"]
mod vm_console;
#[path = "vm/construction.rs"]
mod vm_construction;
#[path = "vm/constructors.rs"]
mod vm_constructors;
#[path = "vm/control.rs"]
mod vm_control;
#[path = "vm/conversions.rs"]
mod vm_conversions;
#[path = "vm/cycle_gc.rs"]
mod vm_cycle_gc;
#[path = "vm/data_view.rs"]
mod vm_data_view;
#[path = "vm/date_ops.rs"]
mod vm_date_ops;
#[path = "vm/disposal.rs"]
mod vm_disposal;
#[path = "vm/error_handling.rs"]
mod vm_error_handling;
#[path = "vm/module_loading.rs"]
mod vm_modules;
#[path = "vm/object_conversion.rs"]
mod vm_object_conversion;
#[path = "vm/process.rs"]
mod vm_process;
#[path = "vm/promise_construction.rs"]
mod vm_promise_construction;
#[path = "vm/property_access.rs"]
mod vm_properties;
#[path = "vm/regexp_ops.rs"]
mod vm_regexp;
#[path = "vm/setup.rs"]
mod vm_setup;
#[path = "vm/weak_refs.rs"]
mod vm_weak_refs;

/// The three reserved slots an `AbortSignal` keeps beside its public shape.
const ABORT_LISTENERS: &str = "@@abort-listeners";

/// A fresh, un-aborted `AbortSignal`.
///
/// An ordinary object rather than an exotic kind: everything a signal does is state plus three
/// methods, and nothing about it needs the value representation to know it exists. `aborted` and
/// `reason` are stored rather than derived so a read is a read, and every member is
/// non-enumerable, which is what keeps `Object.keys(signal)` empty as node's is.
fn abort_signal() -> Value {
    let mut data = ObjectData::default();
    data.set_non_enumerable("aborted", Value::Boolean(false));
    data.set_non_enumerable("reason", Value::Undefined);
    data.set_non_enumerable("onabort", Value::Null);
    data.set_non_enumerable(
        "addEventListener",
        Value::Native(
            Native::AbortSignalListen { remove: false },
            RealmId::PRIMARY,
        ),
    );
    data.set_non_enumerable(
        "removeEventListener",
        Value::Native(Native::AbortSignalListen { remove: true }, RealmId::PRIMARY),
    );
    data.set_non_enumerable(
        "throwIfAborted",
        Value::Native(Native::AbortSignalThrowIfAborted, RealmId::PRIMARY),
    );
    data.set_non_enumerable(ABORT_LISTENERS, Value::Array(ArrayData::new(Vec::new())));
    Value::Object(tracked_object(data))
}

/// `new AbortController()`: a signal, and the one handle that can abort it.
///
/// Handing the two out separately is the whole point -- a callee can be given the signal without
/// being given the power to fire it -- which is the same split `Proxy.revocable` makes.
fn abort_controller() -> Value {
    let mut data = ObjectData::default();
    data.set_non_enumerable("signal", abort_signal());
    data.set_non_enumerable(
        "abort",
        Value::Native(Native::AbortControllerAbort, RealmId::PRIMARY),
    );
    Value::Object(tracked_object(data))
}

/// `new TextEncoder()`. UTF-8 and nothing else, which is all the encoder half is defined to do.
fn text_encoder() -> Value {
    let mut data = ObjectData::default();
    data.set_non_enumerable("encoding", Value::String("utf-8".to_owned().into()));
    data.set_non_enumerable(
        "encode",
        Value::Native(Native::TextEncoderEncode, RealmId::PRIMARY),
    );
    Value::Object(tracked_object(data))
}

fn unary(operator: UnaryOp, value: Value) -> Result<Value, ExecutionError> {
    // A BigInt stays one under negation, bitwise-not and the two updates; `+x` is the exception,
    // and is a TypeError in the specification precisely so that `+` cannot silently narrow one.
    // It was answering with the narrowed number instead, which is the wrong answer the type
    // exists to make impossible.
    if let Value::BigInt(number) = &value {
        let one = || num_bigint::BigInt::from(1_u8);
        return Ok(match operator {
            UnaryOp::Negate => Value::BigInt(Rc::new(-number.as_ref().clone())),
            UnaryOp::BitwiseNot => Value::BigInt(Rc::new(!number.as_ref().clone())),
            UnaryOp::Increment => Value::BigInt(Rc::new(number.as_ref().clone() + one())),
            UnaryOp::Decrement => Value::BigInt(Rc::new(number.as_ref().clone() - one())),
            UnaryOp::ToNumeric => value.clone(),
            UnaryOp::Plus => return Err(ExecutionError::BigIntIsNotANumber),
            _ => unary_of_number(operator, &value),
        });
    }
    // A symbol has no number of its own, and every numeric operator here says so rather than
    // answering the `NaN` that `ToNumber` would otherwise invent.
    if matches!(value, Value::Symbol(_))
        && matches!(
            operator,
            UnaryOp::Plus
                | UnaryOp::Negate
                | UnaryOp::BitwiseNot
                | UnaryOp::Increment
                | UnaryOp::Decrement
                | UnaryOp::ToNumeric
        )
    {
        return Err(ExecutionError::SymbolIsNotANumber);
    }
    Ok(unary_of_number(operator, &value))
}

/// The rest of [`unary`], once neither numeric type is in play.
fn unary_of_number(operator: UnaryOp, value: &Value) -> Value {
    match operator {
        UnaryOp::Plus | UnaryOp::ToNumeric => Value::Number(value.as_number()),
        UnaryOp::Negate => Value::Number(-value.as_number()),
        // `x++` is `ToNumeric(x) + 1`, not `x + 1`: the conversion happens first, which is why
        // `x = "5"; x++` leaves 6 rather than "51".
        UnaryOp::Increment => Value::Number(value.as_number() + 1.0),
        UnaryOp::Decrement => Value::Number(value.as_number() - 1.0),
        UnaryOp::Not => Value::Boolean(!value.is_truthy()),
        // `typeof` never coerces and never throws; it reports the type tag directly.
        UnaryOp::TypeOf => Value::String(value.type_of().to_owned().into()),
        UnaryOp::ToString => unreachable!("abstract ToString requires VM coercion"),
        // `~x` is `-(ToInt32(x) + 1)`: a numeric operator, despite reading as a logical one.
        UnaryOp::BitwiseNot => Value::Number(f64::from(!to_int32(value.as_number()))),
        // `void x` evaluates its operand -- which already happened -- and discards it.
        UnaryOp::Void => Value::Undefined,
    }
}

/// `ToInt32`: the truncation every bitwise operator applies to its operands first.
///
/// This is why `1 << 31` is negative and why `2 ** 32 | 0` is zero. Not a cast: the value wraps
/// modulo 2^32 rather than saturating, and anything not finite becomes zero.
/// SplitMix64: a full-period generator with good statistical properties in five lines, which is
/// what a language builtin needs. Its seed comes from the realm's ADR 0031 entropy capability.
fn split_mix_64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

fn to_int32(value: f64) -> i32 {
    to_uint32(value) as i32
}

/// `ToUint32`, the same truncation read as unsigned. `>>>` is the only operator that uses it
/// directly, which is what makes `x >>> 0` the idiom for reading a number as unsigned.
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    let truncated = value.trunc();
    // `rem_euclid` gives the non-negative remainder, which is the wrap the specification asks
    // for; `as u32` alone saturates instead, so `2 ** 32` would come out as u32::MAX.
    (truncated.rem_euclid(4_294_967_296.0)) as u32
}

/// Arithmetic on two BigInts, or `None` when this is not a BigInt operation.
///
/// Returns `Err` when one side is a BigInt and the other is not: the language refuses that
/// deliberately, since silently widening either way is how precision is lost.
fn bigint_binary(
    operator: BinaryOp,
    left: &Value,
    right: &Value,
) -> Option<Result<Value, ExecutionError>> {
    use num_bigint::BigInt as Big;
    let (a, b) = match (left, right) {
        (Value::BigInt(a), Value::BigInt(b)) => (a.as_ref().clone(), b.as_ref().clone()),
        // `"n = " + 1n` is a *string* operation: `+` reduces both sides first and concatenates
        // when either is a string, and a BigInt has a string spelling like any other value. Only
        // the numeric addition mixes the two, and refusing the concatenation as well made the
        // ordinary way of printing one a TypeError.
        (Value::BigInt(_), Value::String(_)) | (Value::String(_), Value::BigInt(_))
            if matches!(operator, BinaryOp::Add) =>
        {
            return None;
        }
        // Comparison across the two *is* allowed -- only arithmetic is not -- and so is `==`,
        // both of which are handled by the ordinary paths on the numeric value.
        (Value::BigInt(_), _) | (_, Value::BigInt(_)) => {
            return match operator {
                BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Remainder
                | BinaryOp::Exponentiate
                // A shift and a bitwise operator are numeric operators too, and were falling
                // through to the 32-bit path: `1n << 1` answered the *number* 2, which is a
                // BigInt silently narrowed -- the one thing the type exists to prevent.
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight
                | BinaryOp::UnsignedShiftRight
                | BinaryOp::BitwiseAnd
                | BinaryOp::BitwiseOr
                | BinaryOp::BitwiseXor => Some(Err(ExecutionError::MixedBigInt)),
                _ => None,
            };
        }
        _ => return None,
    };
    let zero = Big::from(0);
    Some(Ok(match operator {
        BinaryOp::Add => Value::BigInt(Rc::new(a + b)),
        BinaryOp::Subtract => Value::BigInt(Rc::new(a - b)),
        BinaryOp::Multiply => Value::BigInt(Rc::new(a * b)),
        // Division truncates toward zero and by zero is a RangeError, neither of which matches
        // what a number would do.
        BinaryOp::Divide | BinaryOp::Remainder => {
            if b == zero {
                return Some(Err(ExecutionError::RadixOutOfRange));
            }
            Value::BigInt(Rc::new(if matches!(operator, BinaryOp::Divide) {
                a / b
            } else {
                a % b
            }))
        }
        BinaryOp::Exponentiate => {
            let Some(exponent) = num_traits::ToPrimitive::to_u32(&b) else {
                return Some(Err(ExecutionError::RadixOutOfRange));
            };
            Value::BigInt(Rc::new(num_traits::Pow::pow(a, exponent)))
        }
        BinaryOp::BitwiseAnd => Value::BigInt(Rc::new(a & b)),
        BinaryOp::BitwiseOr => Value::BigInt(Rc::new(a | b)),
        BinaryOp::BitwiseXor => Value::BigInt(Rc::new(a ^ b)),
        // There is no unsigned shift on a BigInt, and it is refused rather than approximated:
        // `>>>` fills from a *fixed* width and a BigInt has none, so there is no answer to give.
        BinaryOp::UnsignedShiftRight => {
            return Some(Err(ExecutionError::TypeErrorMessage {
                message: "BigInts have no unsigned right shift, use >> instead".to_owned(),
            }));
        }
        // One operation with a sign rather than two: a negative count shifts the other way, so
        // `1n << -1n` is `1n >> 1n`. Both had been answering with a 32-bit *number*.
        BinaryOp::ShiftLeft | BinaryOp::ShiftRight => {
            let negative = b < zero;
            let widening = matches!(operator, BinaryOp::ShiftLeft) != negative;
            let count = if negative { -b } else { b };
            let count = num_traits::ToPrimitive::to_u64(&count);
            match count {
                // A shift that would build a number no allocation could hold is refused before
                // anything is allocated. The bound is this engine's rather than the language's,
                // which has none -- but "out of memory" is not an answer a program can catch and
                // a `RangeError` is exactly what every engine raises here.
                Some(count) if widening && count > MAX_BIGINT_SHIFT => {
                    return Some(Err(ExecutionError::RangeErrorMessage {
                        message: "Maximum BigInt size exceeded".to_owned(),
                    }));
                }
                None if widening => {
                    return Some(Err(ExecutionError::RangeErrorMessage {
                        message: "Maximum BigInt size exceeded".to_owned(),
                    }));
                }
                // Narrowing past the end of the value leaves 0 or -1 depending on its sign, which
                // is what an arithmetic shift converges on -- so the count is clamped to the
                // width rather than handed to an allocator as a length.
                Some(count) if !widening => {
                    let clamped = count.min(a.bits().saturating_add(1));
                    Value::BigInt(Rc::new(a >> usize::try_from(clamped).unwrap_or(usize::MAX)))
                }
                None => Value::BigInt(Rc::new(a >> usize::MAX)),
                Some(count) => {
                    Value::BigInt(Rc::new(a << usize::try_from(count).unwrap_or(usize::MAX)))
                }
            }
        }
        BinaryOp::StrictEqual | BinaryOp::LooseEqual => Value::Boolean(a == b),
        BinaryOp::StrictNotEqual | BinaryOp::LooseNotEqual => Value::Boolean(a != b),
        BinaryOp::LessThan => Value::Boolean(a < b),
        BinaryOp::LessThanOrEqual => Value::Boolean(a <= b),
        BinaryOp::GreaterThan => Value::Boolean(a > b),
        BinaryOp::GreaterThanOrEqual => Value::Boolean(a >= b),
        _ => return None,
    }))
}

/// Whether this operator converts each operand as it reaches it, left to right.
///
/// These are the operators with no string reading to choose first: `ApplyStringOrNumericBinary`
/// takes `ToNumeric(lval)` and then `ToNumeric(rval)`, so a refusal on the left is raised before
/// the right operand is reduced at all. `+` is absent because a string on either side makes it
/// concatenation, and the relational operators because two strings compare as text -- both have
/// to reduce each side before they know which conversion they are performing.
fn numeric_left_to_right(operator: BinaryOp) -> bool {
    matches!(
        operator,
        BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Remainder
            | BinaryOp::Exponentiate
            | BinaryOp::BitwiseAnd
            | BinaryOp::BitwiseOr
            | BinaryOp::BitwiseXor
            | BinaryOp::ShiftLeft
            | BinaryOp::ShiftRight
            | BinaryOp::UnsignedShiftRight
    )
}

/// Whether this operator reduces its operands to numbers, which a symbol refuses to become.
///
/// `+` is not on the list because it decides between two conversions -- a string on either side
/// makes it concatenation -- and both refuse a symbol, with different messages. Every other
/// operator here converts unconditionally, which is why `Symbol() - 1` is a `TypeError` rather
/// than the `NaN` that `ToNumber` would have invented for it. Equality is absent for the opposite
/// reason: `Symbol() == 1` converts nothing and is simply false.
fn coerces_to_number(operator: BinaryOp) -> bool {
    matches!(
        operator,
        BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Remainder
            | BinaryOp::Exponentiate
            | BinaryOp::BitwiseAnd
            | BinaryOp::BitwiseOr
            | BinaryOp::BitwiseXor
            | BinaryOp::ShiftLeft
            | BinaryOp::ShiftRight
            | BinaryOp::UnsignedShiftRight
            | BinaryOp::LessThan
            | BinaryOp::LessThanOrEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterThanOrEqual
    )
}

fn binary(
    intrinsics: &Intrinsics,
    operator: BinaryOp,
    left: Value,
    right: Value,
) -> Result<Value, ExecutionError> {
    // A symbol has no number and no string, and the operators that need one say so rather than
    // inventing a value: reading `Symbol() - 1` as `NaN` is the kind of plausible answer that
    // hides the mistake it came from.
    if (matches!(left, Value::Symbol(_)) || matches!(right, Value::Symbol(_)))
        && coerces_to_number(operator)
    {
        return Err(ExecutionError::SymbolIsNotANumber);
    }
    Ok(match operator {
        BinaryOp::Add => {
            if matches!(left, Value::Symbol(_)) || matches!(right, Value::Symbol(_)) {
                // `+` has already reduced both sides, so a symbol here is one the program wrote:
                // it is a string operation when either side is a string and a numeric one
                // otherwise, and a symbol is refused by both -- differently, which is why the
                // two refusals are told apart rather than shared.
                return Err(
                    if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                        ExecutionError::SymbolIsNotAString
                    } else {
                        ExecutionError::SymbolIsNotANumber
                    },
                );
            }
            if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                // A loop that concatenates is the ordinary way a program builds a large string,
                // and the ordinary way it asks for one no engine will give it.
                let (left, right) = (
                    left.to_string_units_checked()?,
                    right.to_string_units_checked()?,
                );
                let total = left.len().saturating_add(right.len());
                bounded_string(total)?;
                Value::String(left.concat(&right))
            } else {
                Value::Number(left.as_number() + right.as_number())
            }
        }
        BinaryOp::Subtract => Value::Number(left.as_number() - right.as_number()),
        BinaryOp::Multiply => Value::Number(left.as_number() * right.as_number()),
        BinaryOp::Divide => Value::Number(left.as_number() / right.as_number()),
        BinaryOp::Remainder => Value::Number(left.as_number() % right.as_number()),
        BinaryOp::Exponentiate => Value::Number(left.as_number().powf(right.as_number())),
        BinaryOp::StrictEqual => Value::Boolean(strict_equal(&left, &right)),
        // `key in object` tests presence, which is not the same question as reading the key and
        // comparing to undefined: a property explicitly set to undefined is present.
        // `in` asks an *object* what it has, and refuses anything else. Answering `false` told a
        // program the property was absent from something that could never have had it -- the
        // same wrong answer for a typo'd receiver as for a deliberate check.
        BinaryOp::In => {
            if !is_an_object(&right) && !is_callable(&right) {
                return Err(ExecutionError::TypeErrorMessage {
                    message: format!(
                        "Cannot use 'in' to search for a property in {}",
                        right.to_display_string()
                    ),
                });
            }
            Value::Boolean(has_property_checked(intrinsics, &right, &left)?)
        }
        // Handled in the interpreter loop, which has the realm's Error.prototype; reaching here
        // would mean the loop stopped special-casing it.
        BinaryOp::InstanceOf => unreachable!("instanceof is evaluated with realm context"),

        BinaryOp::StrictNotEqual => Value::Boolean(!strict_equal(&left, &right)),
        BinaryOp::LooseEqual => Value::Boolean(loose_equal_checked(&left, &right)?),
        BinaryOp::LooseNotEqual => Value::Boolean(!loose_equal_checked(&left, &right)?),
        // Every bitwise operator truncates to 32 bits first, so `1 << 31` is negative.
        BinaryOp::BitwiseAnd => Value::Number(f64::from(
            to_int32(left.as_number()) & to_int32(right.as_number()),
        )),
        BinaryOp::BitwiseOr => Value::Number(f64::from(
            to_int32(left.as_number()) | to_int32(right.as_number()),
        )),
        BinaryOp::BitwiseXor => Value::Number(f64::from(
            to_int32(left.as_number()) ^ to_int32(right.as_number()),
        )),
        // Only the low five bits of the shift count are used, so `1 << 32` is 1, not 0.
        BinaryOp::ShiftLeft => Value::Number(f64::from(
            to_int32(left.as_number()).wrapping_shl(to_uint32(right.as_number()) & 31),
        )),
        BinaryOp::ShiftRight => Value::Number(f64::from(
            to_int32(left.as_number()).wrapping_shr(to_uint32(right.as_number()) & 31),
        )),
        // The one shift that reads its left operand as unsigned, and so the one whose result can
        // exceed i32::MAX -- `(1 << 31) >>> 0` is 2147483648.
        BinaryOp::UnsignedShiftRight => Value::Number(f64::from(
            to_uint32(left.as_number()).wrapping_shr(to_uint32(right.as_number()) & 31),
        )),
        BinaryOp::LessThan => Value::Boolean(compare(&left, &right, OrderingPredicate::Less)),
        BinaryOp::LessThanOrEqual => {
            Value::Boolean(compare(&left, &right, OrderingPredicate::LessEqual))
        }
        BinaryOp::GreaterThan => Value::Boolean(compare(&left, &right, OrderingPredicate::Greater)),
        BinaryOp::GreaterThanOrEqual => {
            Value::Boolean(compare(&left, &right, OrderingPredicate::GreaterEqual))
        }
    })
}

mod serialization;
use serialization::*;
mod object_helpers;
pub use object_helpers::CollectionKey;
use object_helpers::*;
mod intrinsic_helpers;
use intrinsic_helpers::*;
pub use intrinsic_helpers::{ArrayCopy, CallbackMethod, RegExpSymbol, SetOperation};
mod date;
pub use date::DateMethod;
use date::*;
mod builtin_members;
pub use builtin_members::BuiltinAccessor;
use builtin_members::*;

mod properties;
pub use properties::CapabilityRecord;
use properties::*;
mod value_helpers;
use value_helpers::*;
mod binary_data;
pub use binary_data::*;
mod runtime_value;
pub use runtime_value::*;
mod object_storage;
use object_storage::*;
mod value;
use value::*;

#[cfg(test)]
mod tests;

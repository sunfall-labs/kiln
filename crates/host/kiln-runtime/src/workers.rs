//! Cross-realm messages and worker threads: ADR 0070's primitive A (values copied between realms
//! over ports), primitive C's first use (a transferred `ArrayBuffer`'s backing store moves with
//! its message instead of being copied), and the runtime half of `node:worker_threads` (realms
//! issue 06).
//!
//! Everything here is process-wide and `Send`, and nothing here is a JavaScript value. A realm's
//! heap never leaves its thread; what crosses is a [`Message`]: a kind and a flat list of
//! [`Token`]s written either by the shared Rust plain-graph encoder below or by the shared
//! TypeScript clone (`kiln-builtins/src/web_events_streams.ts`) for observable and special
//! values. The receiving realm builds its own values from those tokens; no heap pointer crosses.
//!
//! # Ports
//!
//! An endpoint is one end of a channel: a queue of messages its realm has not taken yet, and a
//! [`Notify`] subscribers that wake the realm's pending [`kiln_host_abi::AsyncHostOp::PortWait`]. Posting
//! from an endpoint appends to its peer's queue. Closing an endpoint tells the peer, which sees
//! the close after the messages already queued for it, as node's `close` event arrives after
//! them. Who owns an endpoint is not recorded: a port transferred to another realm is its number
//! travelling in a message, and the realm that decodes it is the one that waits on it.
//!
//! A broadcast group is a set of endpoints with no peer: posting copies the message to every
//! other member. Groups are named within one *program* -- a top-level realm and the workers it
//! started, and theirs -- never across the process: the realms `KILN_REALMS` and `KILN_WORKERS`
//! thread mode run side by side are separate programs, as node's `cluster` processes are, and a
//! `BroadcastChannel` of one must not reach another (realms issue 25). Ports need no such scope:
//! a port is reached only through its number, which travels in a message of its own program.
//!
//! # Workers
//!
//! [`spawn`] starts a realm on a thread of the process's realm host ([`crate::WorkerPool`]) and
//! runs the engine's closure there. The runtime posts the worker's `online` before the closure
//! runs and its `exit` after, into the parent's end of the worker's channel, so the parent sees
//! them in order with the worker's own messages. [`terminate`] raises a flag the engine checks
//! where it can stop (a turn boundary, an interpreter checkpoint, an `Atomics.wait`) and posts a
//! terminate message to the worker's control endpoint, which wakes a worker that is idle.
//!
//! # `Atomics.wait`
//!
//! [`wait`] and [`notify`] are the waiter list both engines' `Atomics` use across realms: keyed by
//! the shared store and byte offset, compared under the list's lock, so a `notify` after the
//! store that a waiter compared against can never be lost.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use kiln_shared_graph::{Handle as SharedHandle, OpaqueHandle};
use kiln_string::StringUnits;
use tokio::sync::Notify;

/// What a message is, beside its tokens. Only [`kind::USER`] is a program's message; the rest
/// are the worker protocol between the runtime and `node:worker_threads`.
pub mod kind {
    /// A value a program posted.
    pub const USER: u32 = 0;
    /// The worker's thread has started and is about to run its program.
    pub const ONLINE: u32 = 1;
    /// The worker's realm has ended; one number token, the exit code.
    pub const EXIT: u32 = 2;
    /// The worker ended with an exception nothing caught; its tokens describe it.
    pub const ERROR: u32 = 3;
    /// Text the worker wrote to its standard output; one string token.
    pub const STDOUT: u32 = 4;
    /// Text the worker wrote to its standard error; one string token.
    pub const STDERR: u32 = 5;
    /// What a worker starts from (`workerData`, `env`, environment data), queued for the worker
    /// before its thread starts.
    pub const INIT: u32 = 6;
    /// The parent asked the worker to stop (`worker.terminate()`).
    pub const TERMINATE: u32 = 7;
    /// A process signal arrived for a realm listening to it (`crate::signals`); one number
    /// token, the signal.
    pub const SIGNAL: u32 = 8;
}

/// One element of a message.
pub enum Token {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(StringUnits),
    /// A retained node of a sealed user graph. Queue discard drops this strong handle.
    Graph(kiln_shared_graph::Handle),
    /// An `ArrayBuffer`'s backing store, detached from the sending realm and adopted by the
    /// receiving one (ADR 0070 primitive C). What is inside is the engine's own store type; a
    /// process runs one engine, so the receiver downcasts to what the sender boxed.
    Moved(Box<dyn Any + Send>),
    /// An `ArrayBuffer`'s bytes, copied: what a broadcast carries, since every receiver needs a
    /// buffer of its own and a moved store has one owner.
    Bytes(Arc<[u8]>),
    /// A `SharedArrayBuffer`'s store, shared: every realm that receives it sees the same bytes.
    Shared(Arc<dyn Any + Send + Sync>),
}

/// The guarded, side-effect-free view of a value used by the message encoder. Engines only
/// describe their representation; this module owns traversal, references and wire tags.
pub enum PlainNode {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(StringUnits),
    Object { identity: usize, length: usize },
    Array { identity: usize, length: usize },
}

/// One own data field or dense array element. An absent value is an array hole.
pub struct PlainChild<H> {
    pub key: Option<StringUnits>,
    pub value: Option<H>,
}

/// A representation adapter for the common plain-graph message encoder. `describe` and
/// `child` must neither invoke user code nor mutate or allocate in the engine's heap. They
/// return `None` when a node needs the full structured-clone algorithm instead.
pub trait PlainSource {
    type Handle: Copy;

    fn describe(&mut self, handle: Self::Handle) -> Option<PlainNode>;
    fn child(&mut self, handle: Self::Handle, index: usize) -> Option<PlainChild<Self::Handle>>;
}

/// Encode a plain graph in the same token language as the shared TypeScript clone. No message is
/// posted until the entire walk succeeds, so an unsupported descendant falls back intact.
pub fn encode_plain<S: PlainSource>(source: &mut S, root: S::Handle) -> Option<Vec<Token>> {
    fn walk<S: PlainSource>(
        source: &mut S,
        handle: S::Handle,
        seen: &mut HashMap<usize, usize>,
        tokens: &mut Vec<Token>,
        depth: usize,
    ) -> Option<()> {
        if depth > 512 {
            return None;
        }
        match source.describe(handle)? {
            PlainNode::Undefined => tokens.push(Token::Number(0.0)),
            PlainNode::Null => tokens.push(Token::Number(1.0)),
            PlainNode::Bool(value) => tokens.push(Token::Number(if value { 3.0 } else { 2.0 })),
            PlainNode::Number(value) => {
                tokens.push(Token::Number(4.0));
                tokens.push(Token::Number(value));
            }
            PlainNode::String(value) => {
                tokens.push(Token::Number(5.0));
                tokens.push(Token::String(value));
            }
            node @ (PlainNode::Object { identity, length }
            | PlainNode::Array { identity, length }) => {
                if let Some(&reference) = seen.get(&identity) {
                    tokens.push(Token::Number(7.0));
                    tokens.push(Token::Number(reference as f64));
                    return Some(());
                }
                seen.insert(identity, seen.len());
                let object = matches!(node, PlainNode::Object { .. });
                tokens.push(Token::Number(if object { 8.0 } else { 9.0 }));
                tokens.push(Token::Number(length as f64));
                for index in 0..length {
                    let child = source.child(handle, index)?;
                    if object {
                        tokens.push(Token::String(child.key?));
                    }
                    match child.value {
                        Some(value) => walk(source, value, seen, tokens, depth + 1)?,
                        None if !object => tokens.push(Token::Number(10.0)),
                        None => return None,
                    }
                }
            }
        }
        Some(())
    }

    let mut tokens = vec![Token::Number(0.0)];
    walk(source, root, &mut HashMap::new(), &mut tokens, 0)?;
    Some(tokens)
}

impl Token {
    /// A second copy of this token, for a message with several receivers. `None` for a moved
    /// store, which has exactly one.
    pub fn duplicate(&self) -> Option<Token> {
        Some(match self {
            Self::Undefined => Self::Undefined,
            Self::Null => Self::Null,
            Self::Bool(value) => Self::Bool(*value),
            Self::Number(value) => Self::Number(*value),
            Self::String(value) => Self::String(value.clone()),
            Self::Graph(handle) => Self::Graph(handle.clone()),
            Self::Moved(_) => return None,
            Self::Bytes(bytes) => Self::Bytes(Arc::clone(bytes)),
            Self::Shared(store) => Self::Shared(Arc::clone(store)),
        })
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undefined => f.write_str("Undefined"),
            Self::Null => f.write_str("Null"),
            Self::Bool(value) => write!(f, "Bool({value})"),
            Self::Number(value) => write!(f, "Number({value})"),
            Self::String(value) => write!(f, "String({:?})", value.to_lossy_string()),
            Self::Graph(handle) => write!(f, "Graph({:?})", handle.opaque()),
            Self::Moved(_) => f.write_str("Moved(..)"),
            Self::Bytes(bytes) => write!(f, "Bytes({})", bytes.len()),
            Self::Shared(_) => f.write_str("Shared(..)"),
        }
    }
}

/// The tokens of a message carrying one value and no transferred ports, in the layout the
/// TypeScript writes (`web_events_streams.ts`): the port count, then the value's tag and
/// payload. What the runtime's own messages (`online`, `exit`, a worker's output) carry.
pub fn value_tokens(value: Token) -> Vec<Token> {
    const T_UNDEFINED: f64 = 0.0;
    const T_NULL: f64 = 1.0;
    const T_FALSE: f64 = 2.0;
    const T_TRUE: f64 = 3.0;
    const T_NUMBER: f64 = 4.0;
    const T_STRING: f64 = 5.0;
    let ports = Token::Number(0.0);
    match value {
        Token::Undefined => vec![ports, Token::Number(T_UNDEFINED)],
        Token::Null => vec![ports, Token::Number(T_NULL)],
        Token::Bool(flag) => vec![ports, Token::Number(if flag { T_TRUE } else { T_FALSE })],
        Token::Number(_) => vec![ports, Token::Number(T_NUMBER), value],
        Token::String(_) => vec![ports, Token::Number(T_STRING), value],
        other => vec![ports, Token::Number(T_UNDEFINED), other],
    }
}

/// A message: its kind and its tokens.
#[derive(Debug)]
pub struct Message {
    pub kind: u32,
    pub tokens: Vec<Token>,
}

impl Message {
    pub fn new(kind: u32, tokens: Vec<Token>) -> Self {
        Self { kind, tokens }
    }

    fn duplicate(&self) -> Option<Message> {
        let tokens = self
            .tokens
            .iter()
            .map(Token::duplicate)
            .collect::<Option<Vec<_>>>()?;
        Some(Self::new(self.kind, tokens))
    }
}

// ---- Ports ---------------------------------------------------------------------------------

/// A port has messages queued.
pub const STATE_QUEUED: u32 = 1;
/// A port's peer has closed; once its queue is empty the port is closed too.
pub const STATE_PEER_CLOSED: u32 = 2;
/// No such port: never made, or closed.
pub const STATE_GONE: u32 = 4;

struct Endpoint {
    /// The other end of the channel, 0 when there is none (a broadcast member, or a channel
    /// whose other end has closed).
    peer: u64,
    queue: VecDeque<Message>,
    /// Realm completion signals for pending PortWait requests. Posting wakes the realm drive
    /// directly; it checks the endpoint state before completing the request.
    waits: HashMap<u64, Arc<Notify>>,
    peer_closed: bool,
    group: Option<GroupName>,
    /// The parent's end of a running worker's channel: the worker closing its `parentPort`
    /// does not close this end, because the runtime still has the worker's `exit` to deliver
    /// through it.
    awaits_exit: bool,
}

impl Endpoint {
    fn new(peer: u64) -> Self {
        Self {
            peer,
            queue: VecDeque::new(),
            waits: HashMap::new(),
            peer_closed: false,
            group: None,
            awaits_exit: false,
        }
    }

    fn wake(&self) {
        for signal in self.waits.values() {
            signal.notify_one();
        }
    }
}

/// A broadcast group's name: the program it belongs to, and the name that program gave it.
type GroupName = (u64, String);

#[derive(Default)]
struct Ports {
    next: u64,
    next_wait: u64,
    endpoints: HashMap<u64, Endpoint>,
    groups: HashMap<GroupName, Vec<u64>>,
}

impl Ports {
    fn allocate(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn allocate_wait(&mut self) -> u64 {
        self.next_wait += 1;
        self.next_wait
    }
}

fn ports() -> MutexGuard<'static, Ports> {
    static PORTS: OnceLock<Mutex<Ports>> = OnceLock::new();
    lock(PORTS.get_or_init(|| Mutex::new(Ports::default())))
}

/// Take a lock, ignoring poisoning: a realm that panicked has failed its own program, and the
/// queues it shares with every other realm are still queues.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Actor addresses are sealed graph identities. The registry retains the graph and routes only
/// an address whose generation-checked handle is exactly the one registered at spawn. A program
/// can make another shared graph with identical contents, but it cannot manufacture this ID.
struct ActorRoute {
    _address: SharedHandle,
    _owner: SharedHandle,
    sender: u64,
}

static NEXT_ACTOR_REQUEST: AtomicU64 = AtomicU64::new(1);

/// One request namespace is shared by every realm that can call a delegated actor address.
pub fn actor_request_id() -> u64 {
    NEXT_ACTOR_REQUEST.fetch_add(1, Ordering::Relaxed)
}

#[derive(Default)]
struct ActorRegistry {
    routes: HashMap<OpaqueHandle, ActorRoute>,
    // A revoked address cannot be rebound while another realm still holds its graph. Dead
    // graph generations can be discarded, so actor churn does not grow this set forever.
    used: HashSet<OpaqueHandle>,
    closed_since_prune: usize,
}

impl ActorRegistry {
    fn prune_closed(&mut self) {
        if self.closed_since_prune < 1024 {
            return;
        }
        self.used.retain(|token| {
            self.routes.contains_key(token) || SharedHandle::resolve(*token).is_some()
        });
        self.closed_since_prune = 0;
    }
}

fn actor_routes() -> &'static Mutex<ActorRegistry> {
    static ROUTES: OnceLock<Mutex<ActorRegistry>> = OnceLock::new();
    ROUTES.get_or_init(|| Mutex::new(ActorRegistry::default()))
}

/// Bind a fresh capability to one mailbox sender. Once bound, it cannot be redirected.
pub fn actor_register(address: &SharedHandle, owner: &SharedHandle, sender: u64) -> bool {
    if address.opaque() == owner.opaque() || sender == 0 {
        return false;
    }
    let mut registry = actor_routes()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state(sender) & (STATE_GONE | STATE_PEER_CLOSED) != 0 {
        return false;
    }
    if !registry.used.insert(address.opaque()) {
        return false;
    }
    registry.routes.insert(
        address.opaque(),
        ActorRoute {
            _address: address.clone(),
            _owner: owner.clone(),
            sender,
        },
    );
    true
}

/// Return the sending endpoint only while the actor remains registered and its port is open.
pub fn actor_sender(address: &SharedHandle) -> Option<u64> {
    let mut registry = actor_routes()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let route = registry.routes.get(&address.opaque())?;
    if state(route.sender) & (STATE_GONE | STATE_PEER_CLOSED) != 0 {
        registry.routes.remove(&address.opaque());
        registry.closed_since_prune += 1;
        registry.prune_closed();
        return None;
    }
    Some(route.sender)
}

/// Revoke call authority after a stop or an unexpected realm exit.
pub fn actor_close(address: &SharedHandle, owner: &SharedHandle) -> bool {
    let mut registry = actor_routes()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if registry
        .routes
        .get(&address.opaque())
        .is_none_or(|route| route._owner.opaque() != owner.opaque())
    {
        return false;
    }
    registry.routes.remove(&address.opaque());
    registry.closed_since_prune += 1;
    registry.prune_closed();
    true
}

/// A new channel: two entangled endpoints.
pub fn channel() -> (u64, u64) {
    let mut ports = ports();
    let a = ports.allocate();
    let b = ports.allocate();
    ports.endpoints.insert(a, Endpoint::new(b));
    ports.endpoints.insert(b, Endpoint::new(a));
    (a, b)
}

/// A new endpoint with no peer: something the runtime itself posts to with [`enqueue`] (a
/// realm's subscription to a process signal, `crate::signals`).
pub fn endpoint() -> u64 {
    let mut ports = ports();
    let port = ports.allocate();
    ports.endpoints.insert(port, Endpoint::new(0));
    port
}

/// Post `message` from `from` to its peer. `false` when `from` is closed or has no peer, and the
/// message is dropped -- as node drops a message posted to a port whose other end has gone.
pub fn post(from: u64, message: Message) -> bool {
    let mut ports = ports();
    let Some(peer) = ports.endpoints.get(&from).map(|endpoint| endpoint.peer) else {
        drop(ports);
        drop(message);
        return false;
    };
    let Some(target) = ports.endpoints.get_mut(&peer) else {
        drop(ports);
        drop(message);
        return false;
    };
    target.queue.push_back(message);
    target.wake();
    true
}

/// Queue `message` for `port` itself, as though its peer had posted it: what a worker starts
/// from, and messages that were already queued for a port when it was transferred.
pub fn enqueue(port: u64, message: Message) -> bool {
    let mut ports = ports();
    let Some(target) = ports.endpoints.get_mut(&port) else {
        drop(ports);
        drop(message);
        return false;
    };
    target.queue.push_back(message);
    target.wake();
    true
}

/// The oldest message queued for `port`.
pub fn take(port: u64) -> Option<Message> {
    ports().endpoints.get_mut(&port)?.queue.pop_front()
}

/// `STATE_*` bits for `port`.
pub fn state(port: u64) -> u32 {
    let ports = ports();
    let Some(endpoint) = ports.endpoints.get(&port) else {
        return STATE_GONE;
    };
    let mut state = 0;
    if !endpoint.queue.is_empty() {
        state |= STATE_QUEUED;
    }
    if endpoint.peer_closed {
        state |= STATE_PEER_CLOSED;
    }
    state
}

/// Whether a wait on `port` would end now.
pub fn readable(port: u64) -> bool {
    state(port) != 0
}

/// Register one realm's PortWait while holding the queue lock. `None` means a message or close
/// is already visible, including when the endpoint is gone. The same lock makes a post between
/// this check and the registration impossible to miss.
pub fn subscribe(port: u64, signal: Arc<Notify>) -> Option<u64> {
    let mut ports = ports();
    let endpoint = ports.endpoints.get(&port)?;
    if !endpoint.queue.is_empty() || endpoint.peer_closed {
        return None;
    }
    let id = ports.allocate_wait();
    ports.endpoints.get_mut(&port)?.waits.insert(id, signal);
    Some(id)
}

/// Remove a PortWait subscription after delivery, cancellation, or realm shutdown.
pub fn unsubscribe(port: u64, id: u64) {
    if let Some(endpoint) = ports().endpoints.get_mut(&port) {
        endpoint.waits.remove(&id);
    }
}

/// Close `port`: what was queued for it is dropped (a moved store is freed with it), and its
/// peer learns, after the messages already queued for the peer, that this end has gone.
pub fn close(port: u64) {
    let mut ports = ports();
    let Some(endpoint) = ports.endpoints.remove(&port) else {
        return;
    };
    if let Some(peer) = ports.endpoints.get_mut(&endpoint.peer) {
        peer.peer = 0;
        if !peer.awaits_exit {
            peer.peer_closed = true;
            peer.wake();
        }
    }
    if let Some(name) = &endpoint.group
        && let Some(members) = ports.groups.get_mut(name)
    {
        members.retain(|member| *member != port);
        if members.is_empty() {
            ports.groups.remove(name);
        }
    }
    // A wait still parked on this port ends, and finds it gone.
    endpoint.wake();
    drop(ports);
    drop(endpoint);
}

/// Join the broadcast group `name` of this thread's program with a new endpoint.
pub fn broadcast_join(name: &str) -> u64 {
    let group = (program().id, name.to_owned());
    let mut ports = ports();
    let port = ports.allocate();
    let mut endpoint = Endpoint::new(0);
    endpoint.group = Some(group.clone());
    ports.endpoints.insert(port, endpoint);
    ports.groups.entry(group).or_default().push(port);
    port
}

/// Copy `message` to every other member of `from`'s group, in the order they joined; how many
/// received it. A message holding a moved store cannot be copied and reaches nobody.
pub fn broadcast_post(from: u64, message: Message) -> usize {
    let mut ports = ports();
    let Some(name) = ports
        .endpoints
        .get(&from)
        .and_then(|endpoint| endpoint.group.clone())
    else {
        return 0;
    };
    let members = ports.groups.get(&name).cloned().unwrap_or_default();
    let mut delivered = 0;
    for member in members {
        if member == from {
            continue;
        }
        let Some(copy) = message.duplicate() else {
            return delivered;
        };
        if let Some(target) = ports.endpoints.get_mut(&member) {
            target.queue.push_back(copy);
            target.wake();
            delivered += 1;
        }
    }
    delivered
}

// ---- Workers -------------------------------------------------------------------------------

/// What a worker runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerEntry {
    /// A module of the program, by its absolute path.
    File(String),
    /// Source text given with `{ eval: true }`.
    Eval(String),
}

/// Everything the engine needs to start a worker's realm.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    pub entry: WorkerEntry,
    /// The worker's `process.argv`.
    pub argv: Vec<String>,
    /// What the worker's `node:worker_threads` reads about itself (threadId, name, its ports,
    /// resource limits), as the JSON the parent's TypeScript wrote, with `threadId` filled in.
    pub info: String,
    /// The worker's end of its parent channel; `parentPort` in the worker.
    pub port: u64,
    /// The endpoint [`terminate`] posts to, inside the worker.
    pub control: u64,
    /// Whether the worker's standard output and error go to the parent as messages. Always, in
    /// the interpreter; a native realm writes its own streams when the parent does not want them.
    pub pipe_stdio: bool,
}

/// A running worker, as its own thread sees it.
#[derive(Debug)]
pub struct WorkerContext {
    /// Node's `threadId`: numbered from 1 within one program, as node numbers them within one
    /// process (several programs share a process in the daemon and the test runner).
    pub thread_id: u64,
    /// The worker's identity in the runtime, unique in the process: what [`terminate`] and
    /// [`reap`] take.
    pub key: u64,
    /// The program the worker belongs to: its `threadId`s and its broadcast groups. A worker's
    /// own workers belong to it too.
    program: Arc<Program>,
    pub spec: WorkerSpec,
    /// The parent's end of the worker's channel, where the runtime's own messages go.
    parent: u64,
    terminate: Arc<AtomicBool>,
}

impl WorkerContext {
    /// Whether the parent has asked this worker to stop.
    pub fn terminate_requested(&self) -> bool {
        self.terminate.load(Ordering::Acquire)
    }

    /// Send text the worker wrote to one of its standard streams to the parent.
    pub fn write_stdio(&self, stderr: bool, text: StringUnits) {
        let kind = if stderr { kind::STDERR } else { kind::STDOUT };
        enqueue(
            self.parent,
            Message::new(kind, value_tokens(Token::String(text))),
        );
    }

    /// Report the exception that is ending this worker; its tokens are the TypeScript's error
    /// encoding (`web_events_streams.ts`).
    pub fn report_error(&self, tokens: Vec<Token>) {
        enqueue(self.parent, Message::new(kind::ERROR, tokens));
    }
}

struct WorkerRecord {
    terminate: Arc<AtomicBool>,
    control: u64,
    thread: Option<crate::worker_pool::PinnedRealm<()>>,
}

fn workers() -> MutexGuard<'static, HashMap<u64, WorkerRecord>> {
    static WORKERS: OnceLock<Mutex<HashMap<u64, WorkerRecord>>> = OnceLock::new();
    lock(WORKERS.get_or_init(|| Mutex::new(HashMap::new())))
}

/// The realm host every worker's thread comes from (ADR 0070: one host, pinned realms). Sized
/// generously: a worker is a program's request, and the pool's count is a guard against a
/// runaway loop, not a scheduling decision.
fn realm_host() -> &'static crate::WorkerPool {
    static HOST: OnceLock<crate::WorkerPool> = OnceLock::new();
    HOST.get_or_init(|| crate::WorkerPool::new(WORKER_LIMIT))
}

/// Workers one process may run at once.
pub const WORKER_LIMIT: usize = 1024;

/// A worker's thread stack. An interpreter recurses in Rust when a built-in calls back into
/// JavaScript, and a native realm's generated code recurses on the C stack, so a worker gets
/// what a main thread has rather than a thread's default two megabytes. Every thread a native
/// realm runs on gets the same (parallel issue 30): the realms of `kiln_realm_run_pinned`, and the
/// launcher's main thread.
pub const WORKER_STACK: usize = 64 * 1024 * 1024;

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);
static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many workers are running in the process: whether a main thread's `Atomics.wait` can ever
/// be notified by anyone.
pub fn running() -> usize {
    RUNNING.load(Ordering::Acquire)
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<WorkerContext>>> = const { RefCell::new(None) };
    /// Workers this thread's realm started, which end when it does (node: a worker's own
    /// workers do not outlive it, and nothing outlives the process).
    static CHILDREN: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// The current worker's terminate flag, for the checks on hot paths.
    static TERMINATE: Cell<Option<&'static AtomicBool>> = const { Cell::new(None) };
    /// The program a main thread runs; see [`Program`].
    static PROGRAM: RefCell<Option<Arc<Program>>> = const { RefCell::new(None) };
}

/// One program: a top-level realm and every worker under it. What node scopes to a process --
/// `threadId`s numbered from 1, the names `BroadcastChannel`s meet by -- kiln scopes to this,
/// because one kiln process can run several programs at once (`KILN_REALMS`, `KILN_WORKERS` thread
/// mode, the daemon and the test runner), each on its own main thread.
#[derive(Debug)]
struct Program {
    /// Unique in the process: what a broadcast group's name is qualified by.
    id: u64,
    /// The next worker's `threadId`.
    thread_ids: AtomicU64,
}

impl Program {
    fn new() -> Arc<Self> {
        static NEXT_PROGRAM: AtomicU64 = AtomicU64::new(1);
        Arc::new(Self {
            id: NEXT_PROGRAM.fetch_add(1, Ordering::Relaxed),
            thread_ids: AtomicU64::new(1),
        })
    }
}

/// The worker running on this thread, `None` on a main thread.
pub fn current() -> Option<Arc<WorkerContext>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// This thread's `threadId`: 0 on a main thread.
pub fn thread_id() -> u64 {
    CURRENT.with(|current| current.borrow().as_ref().map_or(0, |ctx| ctx.thread_id))
}

/// A new program starts on this (main) thread: its first worker is `threadId` 1 again, and its
/// broadcast groups are its own, as in a new node process.
pub fn begin_program() {
    if current().is_none() {
        PROGRAM.with(|program| *program.borrow_mut() = Some(Program::new()));
    }
}

/// The program this thread's realm belongs to: a worker's is its parent's; a main thread's is
/// the one it began, or one of its own from its first use (each top-level realm of a native
/// process runs on its own thread).
fn program() -> Arc<Program> {
    if let Some(worker) = current() {
        return Arc::clone(&worker.program);
    }
    PROGRAM.with(|program| Arc::clone(program.borrow_mut().get_or_insert_with(Program::new)))
}

fn current_key() -> u64 {
    CURRENT.with(|current| current.borrow().as_ref().map_or(0, |ctx| ctx.key))
}

thread_local! {
    /// The code a worker's `process.exit(code)` asked for, while its realm unwinds to the
    /// launcher that ends it (a native realm has no exception to carry it).
    static EXIT_REQUEST: Cell<Option<i32>> = const { Cell::new(None) };
}

thread_local! {
    /// This main thread's realm is one of several in the process (`KILN_WORKERS` thread mode):
    /// its `process.exit` ends the realm, and the launcher decides what that means for the rest.
    static OWNS_EXIT: Cell<bool> = const { Cell::new(false) };
}

/// Make `process.exit` on this main thread end its realm rather than the process, as it does a
/// worker's: [`request_exit`] records the code and the realm unwinds to its launcher. Called on
/// each realm thread of `KILN_WORKERS` thread mode, where a C `exit` from one realm would tear
/// the process down under the others (realms issue 23).
pub fn own_exits() {
    OWNS_EXIT.with(|owns| owns.set(true));
}

/// A worker's program asked to end with `code` (`process.exit`, or its parent's `terminate`):
/// `false` on a main thread, where ending is the process's to do -- unless the thread's realm
/// owns its exit ([`own_exits`]).
pub fn request_exit(code: i32) -> bool {
    if current().is_none() && !OWNS_EXIT.with(Cell::get) {
        return false;
    }
    EXIT_REQUEST.with(|request| {
        if request.get().is_none() {
            request.set(Some(code));
        }
    });
    true
}

/// Whether this thread's program has asked to end ([`request_exit`]), without taking it.
pub fn exit_requested() -> bool {
    EXIT_REQUEST.with(|request| request.get().is_some())
}

/// The exit a worker's program asked for, if it asked; taken once.
pub fn take_exit_request() -> Option<i32> {
    EXIT_REQUEST.with(Cell::take)
}

/// Whether the worker on this thread has been asked to stop. False on a main thread.
#[inline]
pub fn terminate_requested() -> bool {
    TERMINATE.with(|flag| flag.get().is_some_and(|flag| flag.load(Ordering::Acquire)))
}

/// Start a worker: a thread of the realm host that runs `run` with the worker's context and
/// posts its exit code. Answers the worker's key and the id node calls its `threadId`.
pub fn spawn<F>(spec: WorkerSpec, run: F) -> Result<(u64, u64), String>
where
    F: FnOnce(Arc<WorkerContext>) -> i32 + Send + 'static,
{
    let program = program();
    let thread_id = program.thread_ids.fetch_add(1, Ordering::Relaxed);
    let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
    let terminate = Arc::new(AtomicBool::new(false));
    let control = spec.control;
    let parent = {
        let mut ports = ports();
        let parent = ports
            .endpoints
            .get(&spec.port)
            .map_or(0, |endpoint| endpoint.peer);
        if let Some(endpoint) = ports.endpoints.get_mut(&parent) {
            endpoint.awaits_exit = true;
        }
        parent
    };
    let context = Arc::new(WorkerContext {
        thread_id,
        key,
        program,
        spec,
        parent,
        terminate: Arc::clone(&terminate),
    });
    workers().insert(
        key,
        WorkerRecord {
            terminate: Arc::clone(&terminate),
            control,
            thread: None,
        },
    );
    let body = {
        let context = Arc::clone(&context);
        // A worker belongs to its parent's program: the same `process.pid`, and a signal it
        // sends that program stays in it (realms issue 26).
        let program = crate::signals::current_program();
        move || {
            crate::signals::adopt_program(program);
            run_worker(context, run);
            crate::signals::adopt_program(None);
        }
    };
    RUNNING.fetch_add(1, Ordering::AcqRel);
    match realm_host().spawn_pinned_with_stack(WORKER_STACK, body) {
        Ok(thread) => {
            if let Some(record) = workers().get_mut(&key) {
                record.thread = Some(thread);
            } else {
                // Already finished and forgotten; nothing to join but the thread itself.
                drop(thread.join());
            }
            CHILDREN.with(|children| children.borrow_mut().push(key));
            Ok((key, thread_id))
        }
        Err(error) => {
            RUNNING.fetch_sub(1, Ordering::AcqRel);
            workers().remove(&key);
            Err(error.to_string())
        }
    }
}

fn run_worker<F>(context: Arc<WorkerContext>, run: F)
where
    F: FnOnce(Arc<WorkerContext>) -> i32,
{
    // The flag lives as long as the record and the context that hold it; the thread-local copy
    // is cleared before this function returns, so the `'static` is never outlived.
    let flag: &'static AtomicBool = unsafe { &*Arc::as_ptr(&context.terminate) };
    TERMINATE.with(|slot| slot.set(Some(flag)));
    CURRENT.with(|current| *current.borrow_mut() = Some(Arc::clone(&context)));
    let port = context.spec.port;
    enqueue(
        context.parent,
        Message::new(kind::ONLINE, value_tokens(Token::Undefined)),
    );
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(Arc::clone(&context))))
        .unwrap_or(1);
    // Node reports 1 for a worker it stopped, whatever the worker was doing when it stopped.
    let code = if context.terminate_requested() {
        1
    } else {
        code
    };
    finish_children();
    enqueue(
        context.parent,
        Message::new(kind::EXIT, value_tokens(Token::Number(f64::from(code)))),
    );
    close(port);
    {
        let mut ports = ports();
        if let Some(endpoint) = ports.endpoints.get_mut(&context.parent) {
            endpoint.awaits_exit = false;
            endpoint.peer_closed = true;
            endpoint.wake();
        }
    }
    close(context.spec.control);
    CURRENT.with(|current| *current.borrow_mut() = None);
    TERMINATE.with(|slot| slot.set(None));
    RUNNING.fetch_sub(1, Ordering::AcqRel);
}

/// Ask worker `key` to stop: raise its flag, wake it where it may be waiting, and tell its
/// realm. `false` when there is no such worker (it has already ended).
pub fn terminate(key: u64) -> bool {
    let (terminate, control) = {
        let workers = workers();
        let Some(record) = workers.get(&key) else {
            return false;
        };
        (Arc::clone(&record.terminate), record.control)
    };
    terminate.store(true, Ordering::Release);
    enqueue(
        control,
        Message::new(kind::TERMINATE, value_tokens(Token::Undefined)),
    );
    interrupt_waits(key);
    true
}

/// End every worker this thread's realm started and wait for their threads: the realm is
/// ending, and so do they. A worker that was still running is terminated.
pub fn finish_children() {
    let children = CHILDREN.with(|children| std::mem::take(&mut *children.borrow_mut()));
    for &child in &children {
        terminate(child);
    }
    for child in children {
        let thread = workers()
            .get_mut(&child)
            .and_then(|record| record.thread.take());
        if let Some(thread) = thread {
            drop(thread.join());
        }
        workers().remove(&child);
    }
}

/// Forget workers that have finished, joining their threads: called when a realm learns that one
/// of its workers has exited, so a long-running program does not accumulate joined threads.
pub fn reap(key: u64) {
    let thread = workers()
        .get_mut(&key)
        .and_then(|record| record.thread.take());
    if let Some(thread) = thread {
        drop(thread.join());
    }
    workers().remove(&key);
    CHILDREN.with(|children| children.borrow_mut().retain(|child| *child != key));
}

// ---- Atomics.wait / Atomics.notify ---------------------------------------------------------

/// How a [`wait`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// The value was not the one expected; nothing waited.
    NotEqual,
    /// A [`notify`] woke it.
    Ok,
    TimedOut,
    /// The worker waiting was terminated.
    Interrupted,
}

struct Waiter {
    thread: u64,
    woken: Cell<bool>,
    wake: Condvar,
}

// SAFETY: `woken` is only read or written while `FUTEX`'s mutex is held.
unsafe impl Sync for Waiter {}

type WaiterList = HashMap<(usize, usize), VecDeque<Arc<Waiter>>>;

fn futex() -> &'static Mutex<WaiterList> {
    static FUTEX: OnceLock<Mutex<WaiterList>> = OnceLock::new();
    FUTEX.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Atomics.wait`: if `still_expected()` -- evaluated under the waiter list's lock -- wait on
/// `key` until notified, `timeout` passes, or this worker is terminated. `key` is the shared
/// store's identity and the byte offset of the cell.
pub fn wait(
    key: (usize, usize),
    still_expected: impl FnOnce() -> bool,
    timeout: Option<Duration>,
) -> WaitOutcome {
    let mut list = lock(futex());
    if !still_expected() {
        return WaitOutcome::NotEqual;
    }
    let waiter = Arc::new(Waiter {
        thread: current_key(),
        woken: Cell::new(false),
        wake: Condvar::new(),
    });
    list.entry(key).or_default().push_back(Arc::clone(&waiter));
    let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
    loop {
        if waiter.woken.get() {
            return WaitOutcome::Ok;
        }
        let outcome = if terminate_requested() {
            Some(WaitOutcome::Interrupted)
        } else {
            match deadline {
                Some(deadline) if Instant::now() >= deadline => Some(WaitOutcome::TimedOut),
                _ => None,
            }
        };
        if let Some(outcome) = outcome {
            if let Some(waiters) = list.get_mut(&key) {
                waiters.retain(|other| !Arc::ptr_eq(other, &waiter));
                if waiters.is_empty() {
                    list.remove(&key);
                }
            }
            return outcome;
        }
        list = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                waiter
                    .wake
                    .wait_timeout(list, left)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0
            }
            None => waiter
                .wake
                .wait(list)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        };
    }
}

/// `Atomics.notify`: wake up to `count` waiters on `key`, oldest first; how many were woken.
pub fn notify(key: (usize, usize), count: usize) -> usize {
    let mut list = lock(futex());
    let Some(waiters) = list.get_mut(&key) else {
        return 0;
    };
    let mut woken = 0;
    while woken < count {
        let Some(waiter) = waiters.pop_front() else {
            break;
        };
        waiter.woken.set(true);
        waiter.wake.notify_one();
        woken += 1;
    }
    if waiters.is_empty() {
        list.remove(&key);
    }
    woken
}

fn interrupt_waits(key: u64) {
    let list = lock(futex());
    for waiters in list.values() {
        for waiter in waiters {
            if waiter.thread == key {
                waiter.wake.notify_one();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_shared_graph::{Handle, InputValue, Inspector, Snapshot, SourceId, SourceNode, share};

    struct EmptySharedRecord;

    impl Inspector for EmptySharedRecord {
        type Error = &'static str;

        fn inspect(&mut self, _object: SourceId) -> Result<SourceNode, Self::Error> {
            Ok(SourceNode::Record {
                null_prototype: false,
                properties: Vec::new(),
            })
        }
    }

    struct PlainFixture;

    impl PlainSource for PlainFixture {
        type Handle = usize;

        fn describe(&mut self, handle: usize) -> Option<PlainNode> {
            Some(match handle {
                0 => PlainNode::Object {
                    identity: 1,
                    length: 2,
                },
                1 => PlainNode::Array {
                    identity: 2,
                    length: 3,
                },
                2 => PlainNode::Number(17.0),
                3 => PlainNode::Object {
                    identity: 1,
                    length: 2,
                },
                _ => return None,
            })
        }

        fn child(&mut self, handle: usize, index: usize) -> Option<PlainChild<usize>> {
            match (handle, index) {
                (0, 0) => Some(PlainChild {
                    key: Some("items".into()),
                    value: Some(1),
                }),
                (0, 1) => Some(PlainChild {
                    key: Some("self".into()),
                    value: Some(3),
                }),
                (1, 0) => Some(PlainChild {
                    key: None,
                    value: Some(2),
                }),
                (1, 1) => Some(PlainChild {
                    key: None,
                    value: None,
                }),
                (1, 2) => Some(PlainChild {
                    key: None,
                    value: Some(3),
                }),
                _ => None,
            }
        }
    }

    #[test]
    fn plain_encoder_owns_wire_order_references_and_holes() {
        let tokens = encode_plain(&mut PlainFixture, 0).unwrap();
        let numbers: Vec<_> = tokens
            .iter()
            .filter_map(|token| match token {
                Token::Number(value) => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(
            numbers,
            [0.0, 8.0, 2.0, 9.0, 3.0, 4.0, 17.0, 10.0, 7.0, 0.0, 7.0, 0.0]
        );
        let keys: Vec<_> = tokens
            .iter()
            .filter_map(|token| match token {
                Token::String(value) => Some(value.to_lossy_string()),
                _ => None,
            })
            .collect();
        assert_eq!(keys, ["items", "self"]);
    }

    #[test]
    fn a_message_reaches_the_peer_and_a_close_follows_it() {
        let (a, b) = channel();
        assert!(post(a, Message::new(kind::USER, vec![Token::Number(1.0)])));
        assert_eq!(state(b), STATE_QUEUED);
        close(a);
        assert_eq!(state(b), STATE_QUEUED | STATE_PEER_CLOSED);
        let message = take(b).expect("queued before the close");
        assert!(matches!(message.tokens[..], [Token::Number(n)] if n == 1.0));
        assert_eq!(state(b), STATE_PEER_CLOSED);
        assert!(!post(b, Message::new(kind::USER, Vec::new())));
        close(b);
        assert_eq!(state(b), STATE_GONE);
    }

    #[test]
    fn actor_address_routes_by_shared_identity_and_revokes() {
        struct Empty;
        impl kiln_shared_graph::Inspector for Empty {
            type Error = ();
            fn inspect(&mut self, _: u64) -> Result<kiln_shared_graph::SourceNode, ()> {
                Ok(kiln_shared_graph::SourceNode::Record {
                    null_prototype: false,
                    properties: Vec::new(),
                })
            }
        }
        let address =
            match kiln_shared_graph::share(kiln_shared_graph::InputValue::Object(1), &mut Empty)
                .expect("address graph")
            {
                kiln_shared_graph::Snapshot::Shared(address) => address,
                _ => panic!("address must be shared"),
            };
        let impostor =
            match kiln_shared_graph::share(kiln_shared_graph::InputValue::Object(1), &mut Empty)
                .expect("another empty graph")
            {
                kiln_shared_graph::Snapshot::Shared(address) => address,
                _ => panic!("address must be shared"),
            };
        let (sender, receiver) = channel();
        let owner =
            match kiln_shared_graph::share(kiln_shared_graph::InputValue::Object(1), &mut Empty)
                .expect("owner graph")
            {
                kiln_shared_graph::Snapshot::Shared(owner) => owner,
                _ => panic!("owner must be shared"),
            };
        assert!(actor_register(&address, &owner, sender));
        assert!(!actor_register(&address, &owner, sender));
        assert_eq!(actor_sender(&address), Some(sender));
        assert_eq!(actor_sender(&impostor), None);
        let token = address.opaque();
        drop(address);
        let delegated = kiln_shared_graph::Handle::resolve(token).expect("route retains address");
        assert_eq!(actor_sender(&delegated), Some(sender));
        assert!(!actor_close(&delegated, &impostor));
        assert!(actor_close(&delegated, &owner));
        assert_eq!(actor_sender(&delegated), None);
        assert!(!actor_register(&delegated, &owner, sender));
        close(sender);
        close(receiver);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_port_subscription_wakes_for_post_and_close_and_can_be_cancelled() {
        let (a, b) = channel();
        let signal = Arc::new(Notify::new());
        let first = subscribe(b, Arc::clone(&signal)).expect("empty open port waits");
        assert!(post(a, Message::new(kind::USER, Vec::new())));
        signal.notified().await;
        assert_eq!(state(b), STATE_QUEUED);
        unsubscribe(b, first);
        assert!(
            !ports()
                .endpoints
                .get(&b)
                .unwrap()
                .waits
                .contains_key(&first)
        );
        take(b);
        let second = subscribe(b, Arc::clone(&signal)).expect("drained port waits again");
        close(a);
        signal.notified().await;
        assert_eq!(state(b), STATE_PEER_CLOSED);
        unsubscribe(b, second);
        assert!(subscribe(b, signal).is_none());
        close(b);
    }

    #[test]
    fn discarding_a_queued_shared_graph_releases_its_last_handle() {
        let Snapshot::Shared(handle) = share(InputValue::Object(1), &mut EmptySharedRecord)
            .expect("empty record is shareable")
        else {
            panic!("object root must produce a shared graph");
        };
        let token = handle.opaque();
        let (sender, receiver) = channel();
        assert!(post(
            sender,
            Message::new(kind::USER, vec![Token::Graph(handle.clone())])
        ));
        drop(handle);
        assert!(Handle::resolve(token).is_some());
        close(receiver);
        assert!(Handle::resolve(token).is_none());
        close(sender);
    }

    #[test]
    fn a_broadcast_reaches_every_other_member_with_its_own_copy() {
        let one = broadcast_join("kiln-test-group");
        let two = broadcast_join("kiln-test-group");
        let three = broadcast_join("kiln-test-group");
        let sent = Message::new(kind::USER, vec![Token::Bytes(Arc::from(&[1u8, 2][..]))]);
        assert_eq!(broadcast_post(two, sent), 2);
        assert!(take(one).is_some());
        assert!(take(two).is_none());
        assert!(take(three).is_some());
        for port in [one, two, three] {
            close(port);
        }
    }

    #[test]
    fn a_broadcast_group_is_the_programs_own() {
        // Two top-level realms on two threads are two programs: the same name is two groups.
        let here = broadcast_join("kiln-test-scoped");
        let there = std::thread::spawn(|| broadcast_join("kiln-test-scoped"))
            .join()
            .unwrap();
        let sent = Message::new(kind::USER, vec![Token::Number(1.0)]);
        assert_eq!(broadcast_post(here, sent), 0);
        assert!(take(there).is_none());
        let sent = Message::new(kind::USER, vec![Token::Number(2.0)]);
        assert_eq!(broadcast_post(there, sent), 0);
        assert!(take(here).is_none());

        // A worker belongs to the program that started it, and meets it by name.
        let (parent, port) = channel();
        let (control, _unused) = channel();
        let spec = WorkerSpec {
            entry: WorkerEntry::Eval(String::new()),
            argv: Vec::new(),
            info: String::new(),
            port,
            control,
            pipe_stdio: true,
        };
        let (id, _) = spawn(spec, |_| {
            let member = broadcast_join("kiln-test-scoped");
            let sent = Message::new(kind::USER, vec![Token::Number(3.0)]);
            let delivered = broadcast_post(member, sent);
            close(member);
            i32::try_from(delivered).unwrap_or(-1)
        })
        .unwrap();
        reap(id);
        assert!(matches!(take(here).unwrap().tokens[..], [Token::Number(n)] if n == 3.0));
        assert!(take(there).is_none());
        assert_eq!(take(parent).unwrap().kind, kind::ONLINE);
        let exit = take(parent).unwrap();
        assert!(matches!(exit.tokens[..], [_, _, Token::Number(n)] if n == 1.0));

        // A new program on the same thread starts with groups of its own.
        begin_program();
        let later = broadcast_join("kiln-test-scoped");
        assert_eq!(
            broadcast_post(later, Message::new(kind::USER, Vec::new())),
            0
        );
        assert!(take(here).is_none());
        for port in [here, there, later, parent] {
            close(port);
        }
    }

    #[test]
    fn a_notify_wakes_a_waiter_on_another_thread() {
        let key = (0xfeed, 8);
        let waiter = std::thread::spawn(move || wait(key, || true, None));
        while notify(key, 1) == 0 {
            std::thread::yield_now();
        }
        assert_eq!(waiter.join().unwrap(), WaitOutcome::Ok);
        assert_eq!(wait(key, || false, None), WaitOutcome::NotEqual);
        assert_eq!(
            wait(key, || true, Some(Duration::from_millis(1))),
            WaitOutcome::TimedOut
        );
    }

    #[test]
    fn a_worker_reports_online_then_its_exit_code() {
        let (parent, port) = channel();
        let (control, _unused) = channel();
        let spec = WorkerSpec {
            entry: WorkerEntry::Eval(String::new()),
            argv: Vec::new(),
            info: String::new(),
            port,
            control,
            pipe_stdio: true,
        };
        let (id, _) = spawn(spec, |context| {
            assert!(context.thread_id > 0);
            assert_eq!(thread_id(), context.thread_id);
            7
        })
        .unwrap();
        reap(id);
        assert_eq!(take(parent).unwrap().kind, kind::ONLINE);
        let exit = take(parent).unwrap();
        assert_eq!(exit.kind, kind::EXIT);
        assert!(matches!(exit.tokens[..], [_, _, Token::Number(n)] if n == 7.0));
        assert!(!terminate(id));
        close(parent);
    }
}

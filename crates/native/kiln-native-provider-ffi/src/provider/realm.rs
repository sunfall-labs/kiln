use super::{INVALID_ARGUMENT, RUNTIME_ERROR};
use core::ffi::{CStr, c_char, c_void};
use kiln_host_abi::{
    AsyncHostError, AsyncHostOp, AsyncHostValue, RequestId, ResourceId, ResourceKind,
};
use kiln_runtime::turn_queue::TurnOutcome;
use kiln_runtime::turn_scheduler::{PinnedTurn, PinnedTurnScheduler};
use kiln_runtime::{
    Executor, Realm, RealmCapabilities, RealmState, RuntimeTurn, TimerId, TokioHost, WorkItem,
};
#[cfg(feature = "realm-http")]
use kiln_runtime::{
    HttpCloseMode,
    node_http_agent::{AgentDecision, AgentRelease, HttpOrigin},
};
use kiln_string::StringUnits;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::task::Waker;
use std::time::{Duration, Instant};

#[path = "realm/reclaim.rs"]
mod reclaim;

#[path = "realm/shared_graph.rs"]
mod shared_graph;

type Entry = unsafe extern "C" fn(*mut c_void) -> i32;
type RunWork = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;
type DropWork = unsafe extern "C" fn(*mut c_void);
/// The prelude's promise job lane (Phase 6.3): `run` drains it, `pending` says whether there
/// is anything in it, and the context is the heap the queue lives in.
type RunPromiseLane = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;
type PromiseLanePending = unsafe extern "C" fn(*mut c_void) -> i32;
type RunCompletion =
    unsafe extern "C" fn(*mut c_void, *const CompletionView, *mut c_void, *mut c_void) -> i32;
type RunHttpEvent =
    unsafe extern "C" fn(*mut c_void, *const HttpEventView, *mut c_void, *mut c_void) -> i32;

#[cfg(feature = "realm-http")]
const HTTP_EVENT_REQUEST_HEAD: u32 = 1;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_REQUEST_DATA: u32 = 2;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_REQUEST_END: u32 = 3;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_DRAIN: u32 = 4;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_FINISH: u32 = 5;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_RESPONSE_HEAD: u32 = 6;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_RESPONSE_DATA: u32 = 7;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_RESPONSE_END: u32 = 8;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_ERROR: u32 = 9;
#[cfg(feature = "realm-http")]
const HTTP_EVENT_CLOSED: u32 = 10;
#[cfg(feature = "realm-http")]
const HTTP_AGENT_CONNECT: i32 = 1;
#[cfg(feature = "realm-http")]
const HTTP_AGENT_REUSE: i32 = 2;
#[cfg(feature = "realm-http")]
const HTTP_AGENT_QUEUED: i32 = 3;
#[cfg(feature = "realm-http")]
const HTTP_AGENT_IDLE: i32 = 4;
#[cfg(feature = "realm-http")]
const HTTP_AGENT_DISPATCH: i32 = 5;
#[cfg(feature = "realm-http")]
const HTTP_AGENT_RETIRE: i32 = 6;
#[cfg(feature = "realm-http")]
const HTTP_CLOSE_GRACEFUL: i32 = 0;
#[cfg(feature = "realm-http")]
const HTTP_CLOSE_ABORT: i32 = 1;

#[repr(C)]
pub struct HttpEventView {
    pub kind: u32,
    pub flags: u32,
    pub id: u64,
    pub status: u16,
    pub version_minor: u8,
    pub reserved: u8,
    pub method: *const u8,
    pub method_len: u64,
    pub target: *const u8,
    pub target_len: u64,
    pub reason: *const u8,
    pub reason_len: u64,
    pub headers: *const u8,
    pub headers_len: u64,
    pub data: *const u8,
    pub data_len: u64,
}

#[repr(C)]
pub struct HttpRequestView {
    pub method: *const u8,
    pub method_len: u64,
    pub target: *const u8,
    pub target_len: u64,
    pub headers: *const u8,
    pub headers_len: u64,
    // The current HTTP client path does not need to read the body from this frame, but it is
    // part of the C ABI layout and must remain present so the following fields (and future
    // additions) stay at the offsets the generated C caller expects.
    pub body: *const u8,
    pub body_len: u64,
}

const _: () = assert!(core::mem::size_of::<HttpRequestView>() == 64);

#[repr(C)]
pub struct HttpResponseView {
    pub status: u16,
    pub reserved: u16,
    pub headers: *const u8,
    pub headers_len: u64,
}

/// One owned completion borrowed by a native continuation for the duration of its callback.
///
/// This is private generated-C ABI data. All scalar widths are explicit; pointers are only
/// valid until the callback returns and no Rust or Tokio layout crosses the seam.
#[repr(C)]
pub struct CompletionView {
    pub request: u64,
    pub sequence: u64,
    pub status: i32,
    pub value_kind: u32,
    pub bytes: *const u8,
    pub bytes_len: u64,
    pub stderr: *const u8,
    pub stderr_len: u64,
    pub exit_code: i32,
    pub signal: i32,
    pub value_u64: u64,
    pub resource: u64,
    pub resource_kind: u32,
    pub resource_port: u32,
    pub resource_flags: u32,
    pub address: *const u8,
    pub address_len: u64,
    pub peer_address: *const u8,
    pub peer_address_len: u64,
    pub peer_port: u32,
    pub error_code: *const u8,
    pub error_code_len: u64,
    pub error_syscall: *const u8,
    pub error_syscall_len: u64,
    pub error_path: *const u8,
    pub error_path_len: u64,
    pub error_message: *const u8,
    pub error_message_len: u64,
}

/// Portable operation frame copied by the provider before submission returns.
///
/// `text` carries a path or host, `bytes` carries a write buffer, `resource` names an existing
/// realm resource, and the two scalar arguments carry ports, read sizes, or booleans. The
/// operation tag decides which fields are meaningful; no Rust enum layout crosses this seam.
#[repr(C)]
pub struct HostOpView {
    pub operation: u32,
    pub flags: u32,
    pub resource: u64,
    pub argument: u64,
    pub text: *const u8,
    pub text_len: u64,
    pub bytes: *const u8,
    pub bytes_len: u64,
}

const OP_READ_FILE: u32 = 1;
const OP_WRITE_FILE: u32 = 2;
const OP_STAT: u32 = 3;
const OP_CLOSE_RESOURCE: u32 = 4;
const OP_SET_RESOURCE_REFERENCED: u32 = 5;
const OP_RESOURCE_STATE: u32 = 6;
const OP_TCP_LISTEN: u32 = 7;
const OP_TCP_ACCEPT: u32 = 8;
const OP_TCP_CONNECT: u32 = 9;
const OP_TCP_READ: u32 = 10;
const OP_TCP_WRITE: u32 = 11;
const OP_TCP_SHUTDOWN: u32 = 12;
const OP_STDIN_READ: u32 = 13;
const OP_MKDIR: u32 = 14;
const OP_PROCESS_RUN: u32 = 15;
/// Wait until a cross-realm port has something for this realm (`kiln_runtime::workers`).
const OP_PORT_WAIT: u32 = 16;

/// What a drive answers when the worker it runs has been asked to end (`process.exit`,
/// `terminate`): the C runtime's host-error status, which no `catch` takes, so the realm unwinds
/// to the launcher, which reads the exit code the worker asked for.
const WORKER_EXIT: i32 = -3;

const VALUE_UNDEFINED: u32 = 0;
const VALUE_BYTES: u32 = 1;
const VALUE_FILE_STAT: u32 = 2;
const VALUE_RESOURCE: u32 = 3;
const VALUE_RESOURCE_STATE: u32 = 4;
const VALUE_ACCEPTED_STREAM: u32 = 5;
const VALUE_PROCESS_OUTPUT: u32 = 6;
const VALUE_STRING: u32 = 7;
const VALUE_DIR_ENTRIES: u32 = 8;

const RESOURCE_LISTENER: u32 = 1;
const RESOURCE_STREAM: u32 = 2;
const FLAG_REFERENCED: u32 = 1 << 0;
const FLAG_READ_CLOSED: u32 = 1 << 1;
const FLAG_WRITE_CLOSED: u32 = 1 << 2;
const FLAG_FILE: u32 = 1 << 3;
const FLAG_DIRECTORY: u32 = 1 << 4;

/// What a queued work item keeps alive for the collector (ADR 0041). A payload the C side
/// allocated is itself a heap block; the two Rust-owned payloads name the C block they carry.
#[derive(Clone, Copy)]
enum WorkRoots {
    Block,
    Completion,
}

struct WorkOwner {
    pointer: *mut c_void,
    run: RunWork,
    drop: DropWork,
    roots: WorkRoots,
}

impl Drop for WorkOwner {
    fn drop(&mut self) {
        // SAFETY: the enqueue call transfers one live work allocation and its matching
        // disposer. `Rc` makes this run exactly once, including interval cancellation.
        unsafe { (self.drop)(self.pointer) };
    }
}

/// A timer's work. A timeout runs once and is consumed by its run, so its owner lives inline
/// in the timer entry; only an interval, re-armed with a copy of itself before each run,
/// shares one owner through an `Rc`. With a hundred thousand timeouts pending, the `Rc` was
/// an allocation and a free per timer and one more cold line to read before the run.
enum NativeWork {
    Once(WorkOwner),
    Shared(Rc<WorkOwner>),
}

impl Clone for NativeWork {
    fn clone(&self) -> Self {
        match self {
            Self::Shared(owner) => Self::Shared(Rc::clone(owner)),
            // `Realm::run_next_timer` clones a timer's work only to re-arm an interval, and
            // `set_timer` makes every interval `Shared`; a timeout is never re-armed.
            Self::Once(_) => unreachable!("a one-shot timer's work is never cloned"),
        }
    }
}

/// The promise job queue the prelude owns, registered once per realm heap.
///
/// A promise reaction and an `await` resumption used to arrive here one work item at a time,
/// each a GC cell in the prelude's heap and one call across the private ABI. They never
/// needed anything this side owns -- no deadline, no cancellation, no virtual time -- only a
/// position in the drain, which is what stayed here. The realm asks `pending` and calls
/// `run`; the queue itself, and everything in it, is C.
#[derive(Clone, Copy)]
struct PromiseLane {
    run: RunPromiseLane,
    pending: PromiseLanePending,
    context: *mut c_void,
}

impl PromiseLane {
    fn pending(self) -> bool {
        // SAFETY: registration recorded a live callback pair and the heap they read, and the
        // prelude keeps that heap until the realm's collector tears it down.
        unsafe { (self.pending)(self.context) != 0 }
    }

    fn run(self, host: *mut c_void, thrown: *mut c_void) -> i32 {
        // SAFETY: as above; `host` and `thrown` are owned by the drive in progress.
        unsafe { (self.run)(self.context, host, thrown) }
    }
}

/// A microtask or next-tick job: the same owner as `NativeWork`, held inline. The
/// microtask lanes move a job exactly once, from the queue to the drain, so the `Rc` that
/// lets a timer be re-armed would be a malloc and a free per job for nothing -- and a
/// promise-heavy loop queues one job per `await`.
struct NativeMicrotask(WorkOwner);

impl NativeMicrotask {
    fn new(pointer: *mut c_void, run: Option<RunWork>, drop: Option<DropWork>) -> Option<Self> {
        Self::with_roots(pointer, run, drop, WorkRoots::Block)
    }

    fn with_roots(
        pointer: *mut c_void,
        run: Option<RunWork>,
        drop: Option<DropWork>,
        roots: WorkRoots,
    ) -> Option<Self> {
        let (Some(run), Some(drop)) = (run, drop) else {
            return None;
        };
        if pointer.is_null() {
            return None;
        }
        Some(Self(WorkOwner {
            pointer,
            run,
            drop,
            roots,
        }))
    }

    fn mark_roots(&self, marker: &impl GcRootSink) {
        match self.0.roots {
            WorkRoots::Block => marker.block(self.0.pointer),
            WorkRoots::Completion => {
                let work = unsafe { &*self.0.pointer.cast::<CompletionWork>() };
                if let Some(continuation) = &work.continuation {
                    marker.block(continuation.pointer);
                }
            }
        }
    }

    fn run(&self, host: *mut c_void, thrown: *mut c_void) -> i32 {
        unsafe { (self.0.run)(self.0.pointer, host, thrown) }
    }
}

impl NativeWork {
    fn new(
        pointer: *mut c_void,
        run: Option<RunWork>,
        drop: Option<DropWork>,
        repeating: bool,
    ) -> Option<Self> {
        let (Some(run), Some(drop)) = (run, drop) else {
            return None;
        };
        if pointer.is_null() {
            return None;
        }
        let owner = WorkOwner {
            pointer,
            run,
            drop,
            roots: WorkRoots::Block,
        };
        Some(if repeating {
            Self::Shared(Rc::new(owner))
        } else {
            Self::Once(owner)
        })
    }

    fn owner(&self) -> &WorkOwner {
        match self {
            Self::Once(owner) => owner,
            Self::Shared(owner) => owner,
        }
    }

    fn mark_roots(&self, marker: &impl GcRootSink) {
        let owner = self.owner();
        match owner.roots {
            WorkRoots::Block => marker.block(owner.pointer),
            WorkRoots::Completion => {
                // SAFETY: a Completion work item is always the boxed CompletionWork that
                // `drive` queued, and it stays boxed until its drop callback runs.
                let work = unsafe { &*owner.pointer.cast::<CompletionWork>() };
                if let Some(continuation) = &work.continuation {
                    marker.block(continuation.pointer);
                }
            }
        }
    }

    fn run(&self, host: *mut c_void, thrown: *mut c_void) -> i32 {
        let owner = self.owner();
        // SAFETY: the C Adapter supplied this callback with the allocation, and the caller of
        // `kiln_realm_drive` keeps `host` and `thrown` live for the complete drive.
        unsafe { (owner.run)(owner.pointer, host, thrown) }
    }

    /// Starts bringing this work's payload into the cache: a timer about to run touches the
    /// object its callback settles or calls, which nothing has touched since it was armed.
    fn prefetch(&self) {
        prefetch_read(self.owner().pointer.cast_const());
    }
}

/// A cache hint for the line at `pointer` and the one after it. It reads nothing and cannot
/// fault, whatever `pointer` is.
#[inline(always)]
fn prefetch_read(pointer: *const c_void) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: a prefetch never dereferences its operand architecturally.
    unsafe {
        use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
        _mm_prefetch::<_MM_HINT_T0>(pointer.cast::<i8>());
        _mm_prefetch::<_MM_HINT_T0>(pointer.cast::<i8>().wrapping_add(64));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: as above; `prfm` is a hint and takes no fault.
    unsafe {
        std::arch::asm!(
            "prfm pldl1keep, [{0}]",
            "prfm pldl1keep, [{0}, #64]",
            in(reg) pointer,
            options(nostack, readonly, preserves_flags)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = pointer;
}

struct AsyncContinuation {
    pointer: *mut c_void,
    run: RunCompletion,
    drop: DropWork,
}

#[cfg(feature = "realm-http")]
struct HttpEventOwner {
    pointer: *mut c_void,
    run: RunHttpEvent,
    drop: DropWork,
}

#[cfg(feature = "realm-http")]
impl Drop for HttpEventOwner {
    fn drop(&mut self) {
        // SAFETY: attach transfers one live adapter allocation and its matching disposer.
        unsafe { (self.drop)(self.pointer) };
    }
}

#[cfg(feature = "realm-http")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum HttpRole {
    Server,
    Client,
}

#[cfg(feature = "realm-http")]
struct NativeHttpConnection {
    role: HttpRole,
    events: Rc<HttpEventOwner>,
}

#[cfg(feature = "realm-http")]
fn event_bytes(bytes: &[u8]) -> (*const u8, u64) {
    (
        bytes.as_ptr(),
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
    )
}

impl Drop for AsyncContinuation {
    fn drop(&mut self) {
        // SAFETY: successful request submission transfers the caller's continuation storage
        // and its matching disposer to this realm-owned value.
        unsafe { (self.drop)(self.pointer) };
    }
}

struct CompletionPayload {
    request: u64,
    sequence: u64,
    status: i32,
    value_kind: u32,
    bytes: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
    signal: i32,
    value_u64: u64,
    resource: u64,
    resource_kind: u32,
    resource_port: u32,
    resource_flags: u32,
    address: Vec<u8>,
    peer_address: Vec<u8>,
    peer_port: u32,
    error_code: Vec<u8>,
    error_syscall: Vec<u8>,
    error_path: Vec<u8>,
    error_message: Vec<u8>,
}

impl CompletionPayload {
    fn empty(request: RequestId, sequence: u64, value_kind: u32) -> Self {
        Self {
            request: request.get(),
            sequence,
            status: 0,
            value_kind,
            bytes: Vec::new(),
            stderr: Vec::new(),
            exit_code: i32::MIN,
            signal: i32::MIN,
            value_u64: 0,
            resource: 0,
            resource_kind: 0,
            resource_port: 0,
            resource_flags: 0,
            address: Vec::new(),
            peer_address: Vec::new(),
            peer_port: 0,
            error_code: Vec::new(),
            error_syscall: Vec::new(),
            error_path: Vec::new(),
            error_message: Vec::new(),
        }
    }

    fn resource_kind(kind: ResourceKind) -> u32 {
        match kind {
            ResourceKind::TcpListener => RESOURCE_LISTENER,
            ResourceKind::TcpStream => RESOURCE_STREAM,
        }
    }

    fn install_resource(&mut self, resource: kiln_host_abi::ResourceInfo) {
        self.resource = resource.id.get();
        self.resource_kind = Self::resource_kind(resource.kind);
        self.resource_port = resource.port.map_or(0, u32::from);
        self.address = resource.address.unwrap_or_default().into_bytes();
    }

    fn from_result(
        request: RequestId,
        sequence: u64,
        result: Result<AsyncHostValue, AsyncHostError>,
    ) -> Self {
        match result {
            Ok(AsyncHostValue::Undefined) => Self::empty(request, sequence, VALUE_UNDEFINED),
            Ok(AsyncHostValue::Bytes(bytes)) => {
                let mut payload = Self::empty(request, sequence, VALUE_BYTES);
                payload.bytes = bytes;
                payload
            }
            Ok(AsyncHostValue::ProcessOutput(output)) => {
                let mut payload = Self::empty(request, sequence, VALUE_PROCESS_OUTPUT);
                payload.bytes = output.stdout;
                payload.stderr = output.stderr;
                payload.exit_code = output.exit_code.unwrap_or(i32::MIN);
                payload.signal = output.signal.unwrap_or(i32::MIN);
                payload
            }
            Ok(AsyncHostValue::FileStat(stat)) => {
                let mut payload = Self::empty(request, sequence, VALUE_FILE_STAT);
                payload.value_u64 = stat.size;
                payload.resource_flags = if stat.is_file { FLAG_FILE } else { 0 }
                    | if stat.is_directory { FLAG_DIRECTORY } else { 0 };
                payload
            }
            Ok(AsyncHostValue::Resource(resource)) => {
                let mut payload = Self::empty(request, sequence, VALUE_RESOURCE);
                payload.install_resource(resource);
                payload
            }
            Ok(AsyncHostValue::ResourceState(state)) => {
                let mut payload = Self::empty(request, sequence, VALUE_RESOURCE_STATE);
                payload.install_resource(state.info);
                payload.resource_flags = if state.referenced { FLAG_REFERENCED } else { 0 }
                    | if state.read_closed {
                        FLAG_READ_CLOSED
                    } else {
                        0
                    }
                    | if state.write_closed {
                        FLAG_WRITE_CLOSED
                    } else {
                        0
                    };
                payload
            }
            Ok(AsyncHostValue::AcceptedTcpStream(accepted)) => {
                let mut payload = Self::empty(request, sequence, VALUE_ACCEPTED_STREAM);
                payload.install_resource(accepted.resource);
                payload.peer_address = accepted.peer_address.into_bytes();
                payload.peer_port = u32::from(accepted.peer_port);
                payload
            }
            Ok(AsyncHostValue::String(string)) => {
                let mut payload = Self::empty(request, sequence, VALUE_STRING);
                payload.bytes = string.into_bytes();
                payload
            }
            Ok(AsyncHostValue::DirEntries(_)) => Self::empty(request, sequence, VALUE_DIR_ENTRIES),
            Err(error) => Self::error(
                request,
                sequence,
                &error.code,
                &error.syscall,
                error.path.as_deref(),
                &error.message,
            ),
        }
    }

    fn error(
        request: RequestId,
        sequence: u64,
        code: &str,
        syscall: &str,
        path: Option<&str>,
        message: &str,
    ) -> Self {
        Self {
            request: request.get(),
            sequence,
            status: 1,
            value_kind: 0,
            bytes: Vec::new(),
            stderr: Vec::new(),
            exit_code: i32::MIN,
            signal: i32::MIN,
            value_u64: 0,
            resource: 0,
            resource_kind: 0,
            resource_port: 0,
            resource_flags: 0,
            address: Vec::new(),
            peer_address: Vec::new(),
            peer_port: 0,
            error_code: code.as_bytes().to_vec(),
            error_syscall: syscall.as_bytes().to_vec(),
            error_path: path.unwrap_or_default().as_bytes().to_vec(),
            error_message: message.as_bytes().to_vec(),
        }
    }

    fn bytes_ptr_len(bytes: &[u8]) -> (*const u8, u64) {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        (bytes.as_ptr(), length)
    }

    fn view(&self) -> CompletionView {
        let (bytes, bytes_len) = Self::bytes_ptr_len(&self.bytes);
        let (stderr, stderr_len) = Self::bytes_ptr_len(&self.stderr);
        let (address, address_len) = Self::bytes_ptr_len(&self.address);
        let (peer_address, peer_address_len) = Self::bytes_ptr_len(&self.peer_address);
        let (error_code, error_code_len) = Self::bytes_ptr_len(&self.error_code);
        let (error_syscall, error_syscall_len) = Self::bytes_ptr_len(&self.error_syscall);
        let (error_path, error_path_len) = Self::bytes_ptr_len(&self.error_path);
        let (error_message, error_message_len) = Self::bytes_ptr_len(&self.error_message);
        CompletionView {
            request: self.request,
            sequence: self.sequence,
            status: self.status,
            value_kind: self.value_kind,
            bytes,
            bytes_len,
            stderr,
            stderr_len,
            exit_code: self.exit_code,
            signal: self.signal,
            value_u64: self.value_u64,
            resource: self.resource,
            resource_kind: self.resource_kind,
            resource_port: self.resource_port,
            resource_flags: self.resource_flags,
            address,
            address_len,
            peer_address,
            peer_address_len,
            peer_port: self.peer_port,
            error_code,
            error_code_len,
            error_syscall,
            error_syscall_len,
            error_path,
            error_path_len,
            error_message,
            error_message_len,
        }
    }
}

struct CompletionWork {
    request: RequestId,
    continuation: Option<AsyncContinuation>,
    payload: CompletionPayload,
    cancelled: Rc<RefCell<HashSet<RequestId>>>,
    queued: Rc<RefCell<HashSet<RequestId>>>,
}

unsafe extern "C" fn run_completion(
    pointer: *mut c_void,
    host: *mut c_void,
    thrown: *mut c_void,
) -> i32 {
    if pointer.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the pointer is owned by the NativeWork transferred to the realm queue.
    let work = unsafe { &mut *pointer.cast::<CompletionWork>() };
    work.queued.borrow_mut().remove(&work.request);
    if work.cancelled.borrow_mut().remove(&work.request) {
        work.continuation.take();
        return 0;
    }
    let Some(continuation) = work.continuation.take() else {
        return RUNTIME_ERROR;
    };
    let view = work.payload.view();
    // SAFETY: the callback receives a view whose backing vectors remain owned by `work` until
    // it returns. The host and thrown slots are owned by the complete realm drive.
    unsafe { (continuation.run)(continuation.pointer, &raw const view, host, thrown) }
}

unsafe extern "C" fn drop_completion(pointer: *mut c_void) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: NativeWork invokes this exactly once for the boxed completion payload.
    unsafe { drop(Box::from_raw(pointer.cast::<CompletionWork>())) };
}

/// A borrowed unit view for hashbrown's `Equivalent` lookup. The map owns `StringUnits`;
/// this query object lets an existing Symbol.for key probe it without allocating.
struct BorrowedStringView<'a>(kiln_string::StringView<'a>);

impl Hash for BorrowedStringView<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl hashbrown::Equivalent<StringUnits> for BorrowedStringView<'_> {
    fn equivalent(&self, key: &StringUnits) -> bool {
        self.0 == key.as_view()
    }
}

struct ScheduledWait {
    wake: Rc<Cell<Option<kiln_runtime::DriveWake>>>,
    yield_first: bool,
    timer_armed: Rc<Cell<bool>>,
    virtual_delay: Option<Duration>,
    advanced: bool,
}

/// Reasons a scheduled realm must keep its current OS lane. These are deliberately lifetime
/// pins: an operation can leave a callback or Tokio task reachable after the submitting turn,
/// and proving its final release requires an owner-specific audit.
const AFFINITY_LOCAL_TASK: u32 = 1 << 0;
const AFFINITY_HOST_OP: u32 = 1 << 1;
const AFFINITY_HTTP: u32 = 1 << 2;
#[cfg(feature = "ffi")]
const AFFINITY_FFI: u32 = 1 << 3;
pub(crate) const AFFINITY_ATOMICS_WAIT: u32 = 1 << 4;

struct NativeRealm {
    runtime: NativeRuntime,
    work: RefCell<Realm<NativeMicrotask, NativeWork>>,
    continuations: RefCell<HashMap<RequestId, AsyncContinuation>>,
    #[cfg(feature = "realm-http")]
    http_connections: RefCell<HashMap<kiln_host_abi::ResourceId, NativeHttpConnection>>,
    #[cfg(feature = "realm-http")]
    next_http_client_request_id: Cell<u64>,
    cancelled: Rc<RefCell<HashSet<RequestId>>>,
    queued: Rc<RefCell<HashSet<RequestId>>>,
    symbols: RefCell<hashbrown::HashMap<StringUnits, u64, std::collections::hash_map::RandomState>>,
    symbol_keys: RefCell<HashMap<u64, StringUnits>>,
    callable_stores: RefCell<HashMap<(u32, u64), CallableStore>>,
    value_slots: RefCell<HashMap<u32, Box<[u64; 4]>>>,
    /// The command line this realm's launcher was started with, copied out of `main`'s
    /// `argv` before the realm is entered. Each element is its own allocation so that a
    /// pointer handed to generated C stays put for as long as the realm holds the vector.
    argv: RefCell<Vec<Box<[u8]>>>,
    entropy: Cell<Option<(u64, u64)>>,
    entered: Cell<bool>,
    diagnostic: RefCell<RealmDiagnostic>,
    /// The `host` and `thrown` slots of the drive in progress, for callbacks the HTTP engine
    /// makes from inside a Tokio task (ADR 0046). Null outside a drive.
    drive_context: Cell<(*mut c_void, *mut c_void)>,
    /// Which clock this realm keeps (ADR 0044): the deterministic virtual clock unless the
    /// launcher asked for the host's wall clock before entering the realm.
    clock: Cell<NativeClock>,
    /// The prelude's promise job queue, registered when its heap attached. `None` until
    /// then, and for a realm whose generated C never asked for a heap.
    promise_lane: Cell<Option<PromiseLane>>,
    /// Whether the prelude has a `SIGUSR1` handler installed, as of the last turn boundary.
    /// A wall-clock realm bounds its park when this is set, so an idle server answers a
    /// signal promptly instead of at its next request.
    introspect_armed: Cell<bool>,
    /// A scheduled drive's local waiter. Only its owning lane polls the task or reads this
    /// state; the shared reactor can wake its ticket without seeing the realm itself.
    scheduled_wait: RefCell<Option<ScheduledWait>>,
    /// Zero only while a realm has never published thread-affine state.
    affinity: Cell<u32>,
    /// Program identity, children and exit state follow this realm across scheduler lanes.
    worker_scope: Rc<RefCell<kiln_runtime::workers::RealmScopeState>>,
    /// The realm's `node:ffi` libraries, prepared functions and callback closures (ADR 0070):
    /// released at teardown before the heap their callbacks name is torn down.
    #[cfg(feature = "ffi")]
    ffi: RefCell<crate::ffi::State>,
    /// Declared last so it is dropped last: `Drop for NativeRealm` tears the heap down only
    /// after every field above has released what it held.
    gc: RefCell<GcState>,
}

/// The clock a realm keeps (ADR 0044).
#[derive(Clone, Copy)]
enum NativeClock {
    /// ADR 0012's virtual clock: `now` advances only when a timer runs, and Tokio time is
    /// paused so a distant timer costs nothing. Every realm starts here; it is the clock the
    /// conformance harness compares.
    Virtual,
    /// The host's wall clock: `now` is Unix time, advanced at every turn boundary, and timers
    /// wait on live Tokio time so they fire on schedule while the realm is busy.
    Wall {
        epoch: std::time::Duration,
        started: std::time::Instant,
    },
}

impl NativeClock {
    fn wall() -> Self {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::ZERO);
        Self::Wall {
            epoch,
            started: std::time::Instant::now(),
        }
    }

    /// The host's reading now, or `None` for a virtual realm, whose clock is its own.
    fn reading(self) -> Option<std::time::Duration> {
        match self {
            Self::Virtual => None,
            Self::Wall { epoch, started } => Some(epoch.saturating_add(started.elapsed())),
        }
    }
}

/// A turn boundary for the realm's clock: a wall-clock realm reads the host now, so timers
/// armed by the callbacks that follow measure from this instant, as Node's loop time does.
/// A virtual realm's clock is not touched.
fn tick_clock(realm: &NativeRealm) {
    if let Some(now) = realm.clock.get().reading()
        && let Ok(mut work) = realm.work.try_borrow_mut()
    {
        work.advance_to(now);
    }
}

/// What the realm's clock reads: the host's wall clock for a wall-clock realm, otherwise the
/// realm's virtual instant. `None` only while the virtual clock is mid-mutation.
fn realm_clock_reading(realm: &NativeRealm) -> Option<std::time::Duration> {
    match realm.clock.get().reading() {
        Some(now) => Some(now),
        None => realm.work.try_borrow().ok().map(|work| work.now()),
    }
}

#[derive(Default)]
struct RealmDiagnostic {
    site: Vec<u8>,
    operation: Vec<u8>,
    detail: Vec<u8>,
    function: Vec<u8>,
    instruction: u64,
    depth: u64,
    method: u64,
    receiver_tag: u64,
    has_method: bool,
}

struct CallableStore {
    pointer: *mut c_void,
    drop: DropWork,
}

type GcRootsFn = unsafe extern "C" fn(*mut c_void, *mut c_void, i32);
type GcVisitRoot = unsafe extern "C" fn(*mut c_void, *const c_void, u32) -> i32;
type GcRootsVisitFn = unsafe extern "C" fn(*mut c_void, GcVisitRoot, *mut c_void) -> i32;

/// The prelude's collector entry points (ADR 0041). The prelude owns the heap layout and the
/// tracer; the archive only ever calls these and reads the hooks pointer that every heap keeps
/// as its first word.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GcHooks {
    version: u32,
    size: u32,
    collect: Option<unsafe extern "C" fn(*mut c_void, i32)>,
    teardown_begin: Option<unsafe extern "C" fn(*mut c_void)>,
    teardown_end: Option<unsafe extern "C" fn(*mut c_void)>,
    mark_value: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    mark_block: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    /// Offer the prelude the same turn boundary the collector sweeps at, for the `SIGUSR1`
    /// dump. Returns a bitmask; [`INTROSPECT_ARMED`] is the only bit the archive reads, the
    /// rest saying what the prelude did with the offer.
    introspect: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
}

const GC_HOOKS_VERSION: u32 = 2;
/// The process installed a `SIGUSR1` handler, so a park has to stay short enough to notice it.
const INTROSPECT_ARMED: i32 = 1;
const GC_PHASE_MARK: i32 = 0;
const GC_PHASE_TEARDOWN: i32 = 1;
const GC_PHASE_RESET: i32 = 2;
const GC_PHASE_LANE_RESET: i32 = 3;
const GC_ROOT_SURVIVES_TEARDOWN: u32 = 1;

struct GcRoot {
    roots: GcRootsFn,
    visit: Option<GcRootsVisitFn>,
    context: *mut c_void,
    flags: u32,
}

struct GcState {
    heap: *mut c_void,
    hooks: Option<GcHooks>,
    roots: Vec<GcRoot>,
}

impl Default for GcState {
    fn default() -> Self {
        Self {
            heap: std::ptr::null_mut(),
            hooks: None,
            roots: Vec::new(),
        }
    }
}

struct GcMarker {
    heap: *mut c_void,
    hooks: GcHooks,
}

trait GcRootSink {
    fn block(&self, pointer: *mut c_void);
    fn value(&self, value: *const c_void);
}

impl GcRootSink for GcMarker {
    fn block(&self, pointer: *mut c_void) {
        if pointer.is_null() {
            return;
        }
        if let Some(mark) = self.hooks.mark_block {
            // SAFETY: the prelude installed this hook for exactly this heap.
            unsafe { mark(self.heap, pointer) };
        }
    }

    fn value(&self, value: *const c_void) {
        if value.is_null() {
            return;
        }
        if let Some(mark) = self.hooks.mark_value {
            // SAFETY: the prelude installed this hook for exactly this heap.
            unsafe { mark(self.heap, value) };
        }
    }
}

const GC_VISIT_BLOCK: u32 = 1;
const GC_VISIT_VALUE: u32 = 2;
const GC_VISIT_REGISTERED_ROOTS_UNENUMERATED: i32 = 1;

struct GcVisitor {
    visit: GcVisitRoot,
    context: *mut c_void,
    failed: std::cell::Cell<bool>,
}

unsafe extern "C" fn relay_registered_root(
    context: *mut c_void,
    pointer: *const c_void,
    kind: u32,
) -> i32 {
    // SAFETY: `kiln_realm_gc_visit_provider_roots` keeps this visitor live until all registered
    // callbacks return. The callback receives a read-only borrow and cannot retain it.
    let visitor = unsafe { &*context.cast::<GcVisitor>() };
    if visitor.failed.get() {
        return RUNTIME_ERROR;
    }
    // SAFETY: the caller supplied this visitor and its context for the duration of the walk.
    let status = unsafe { (visitor.visit)(visitor.context, pointer, kind) };
    if status != 0 {
        visitor.failed.set(true);
    }
    status
}

impl GcRootSink for GcVisitor {
    fn block(&self, pointer: *mut c_void) {
        if pointer.is_null() || self.failed.get() {
            return;
        }
        // SAFETY: the caller guarantees the visitor and context remain valid for this call.
        if unsafe { (self.visit)(self.context, pointer.cast_const(), GC_VISIT_BLOCK) } != 0 {
            self.failed.set(true);
        }
    }

    fn value(&self, value: *const c_void) {
        if value.is_null() || self.failed.get() {
            return;
        }
        // SAFETY: the caller guarantees the visitor and context remain valid for this call.
        if unsafe { (self.visit)(self.context, value, GC_VISIT_VALUE) } != 0 {
            self.failed.set(true);
        }
    }
}

fn gc_hooks_of(heap: *mut c_void) -> Option<GcHooks> {
    if heap.is_null() {
        return None;
    }
    // SAFETY: ADR 0041 fixes the heap's first word as the hooks pointer.
    let hooks = unsafe { *heap.cast::<*const GcHooks>() };
    if hooks.is_null() {
        return None;
    }
    // SAFETY: the prelude keeps its hooks table static for the life of the program.
    Some(unsafe { *hooks })
}

/// Enumerates every root the archive itself keeps, for both GC marking and the read-only
/// incoming-edge probe. Registered C callbacks are visited separately.
fn for_each_provider_root(realm: &NativeRealm, sink: &impl GcRootSink) {
    for slot in realm.value_slots.borrow().values() {
        sink.value(slot.as_ptr().cast::<c_void>());
    }
    for store in realm.callable_stores.borrow().values() {
        sink.block(store.pointer);
    }
    for continuation in realm.continuations.borrow().values() {
        sink.block(continuation.pointer);
    }
    #[cfg(feature = "realm-http")]
    for connection in realm.http_connections.borrow().values() {
        sink.block(connection.events.pointer);
    }
    {
        let work = realm.work.borrow();
        work.for_each_queued(|item| item.mark_roots(sink));
        work.for_each_timer(|item| item.mark_roots(sink));
    }
}

fn mark_realm_roots(realm: &NativeRealm, heap: *mut c_void) -> i32 {
    let (hooks, roots) = {
        let gc = realm.gc.borrow();
        if gc.heap.is_null() || gc.heap != heap {
            return INVALID_ARGUMENT;
        }
        let Some(hooks) = gc.hooks else {
            return INVALID_ARGUMENT;
        };
        let roots: Vec<(GcRootsFn, *mut c_void)> = gc
            .roots
            .iter()
            .map(|root| (root.roots, root.context))
            .collect();
        (hooks, roots)
    };
    let marker = GcMarker { heap, hooks };
    for_each_provider_root(realm, &marker);
    for (roots, context) in roots {
        // SAFETY: registration recorded a live root function and the context it asked for.
        unsafe { roots(heap, context, GC_PHASE_MARK) };
    }
    0
}

/// Offers the collector a turn boundary: no generated frame is on the stack when the drive
/// loop calls this, so the prelude may sweep. The prelude decides whether it is due.
fn collect_if_due(realm: &NativeRealm) {
    let (heap, collect) = {
        let gc = realm.gc.borrow();
        match (gc.heap, gc.hooks.as_ref().and_then(|hooks| hooks.collect)) {
            (heap, Some(collect)) if !heap.is_null() => (heap, collect),
            _ => return,
        }
    };
    // SAFETY: the prelude installed this hook for this heap, and no RefCell of the realm is
    // borrowed across the call, so the root walk it performs can borrow them.
    unsafe { collect(heap, 0) };
}

/// Offers the prelude the same turn boundary, for the `SIGUSR1` dump. The prelude owns both
/// the flag the handler set and the format; all this decides is when it is safe to ask.
///
/// The answer also says whether a handler is installed at all, which the park below needs:
/// an idle server that parks forever would answer a signal only once it stopped being idle.
fn introspect_if_requested(realm: &NativeRealm) {
    let (heap, introspect) = {
        let gc = realm.gc.borrow();
        match (
            gc.heap,
            gc.hooks.as_ref().and_then(|hooks| hooks.introspect),
        ) {
            (heap, Some(introspect)) if !heap.is_null() => (heap, introspect),
            _ => return,
        }
    };
    // SAFETY: the prelude installed this hook for this heap, and the dump calls back into
    // this archive, so no RefCell of the realm may be borrowed across the call.
    let status = unsafe { introspect(heap) };
    realm
        .introspect_armed
        .set(status & INTROSPECT_ARMED == INTROSPECT_ARMED);
}

/// Makes a heap this thread's current one (`kiln_realm_gc_current_heap`) until dropped, then
/// puts back whatever was current before. Both changes bump the realm generation, so the
/// prelude's per-thread caches of "which heap" (`kiln_c99_gc_heap`) ask again.
struct CurrentHeap {
    previous: *mut c_void,
}

impl CurrentHeap {
    fn set(heap: *mut c_void) -> Self {
        let previous = ACTIVE_HEAP.with(|slot| slot.replace(heap));
        bump_generation();
        Self { previous }
    }
}

impl Drop for CurrentHeap {
    fn drop(&mut self) {
        ACTIVE_HEAP.with(|slot| slot.set(self.previous));
        bump_generation();
    }
}

impl Drop for NativeRealm {
    fn drop(&mut self) {
        // A scheduled main realm may have started dedicated workers on a lane shared with
        // other realms. End only its own children while its program scope is installed.
        // A dedicated realm may be dropped by the TLS registry destructor after other TLS
        // keys have already gone away. `try_with` makes that path a no-op.
        if SCHEDULED_POOL
            .try_with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade).is_some())
            .unwrap_or(false)
            && kiln_runtime::workers::current().is_none()
        {
            let _scope =
                kiln_runtime::workers::RealmScopeGuard::enter(Rc::clone(&self.worker_scope));
            kiln_runtime::workers::finish_children();
        }
        // Teardown runs prelude code -- the callable stores' and work items' destructors below,
        // the collector's teardown hooks -- and the prelude finds its heap by asking which one is
        // current on this thread. A realm is destroyed outside `ActiveRealm`, so nothing is: the
        // prelude then answered with its fallback heap, a single process static made on first use
        // (realms issue 24). Each realm thread tearing down at the same moment initialised and
        // used that one heap at once -- one thread zeroed it while another was laying out its
        // first nursery chunk, and the collector segfaulted on a zero chunk size -- and every
        // release below went to a heap that had never held the block. So the realm's own heap is
        // current for the whole of teardown, and the thread's previous answer comes back after.
        let heap = self.gc.borrow().heap;
        let current = (!heap.is_null()).then(|| CurrentHeap::set(heap));
        // Release what the archive holds first, while the heap is still alive: a queued work
        // item or a continuation drops through a C callback that may release its block. Then
        // mark only the roots that must outlive the realm (a borrowed result slot) and free
        // the rest, and finally let every unit reset the statics that pointed into the heap.
        *self.work.borrow_mut() = Realm::new();
        self.continuations.borrow_mut().clear();
        #[cfg(feature = "realm-http")]
        self.http_connections.borrow_mut().clear();
        self.callable_stores.borrow_mut().clear();
        self.value_slots.borrow_mut().clear();
        // Close the realm's libraries and free its callback closures while the heap their
        // userdata points into still exists; nothing can call one after this.
        #[cfg(feature = "ffi")]
        drop(std::mem::take(&mut *self.ffi.borrow_mut()));
        let gc = std::mem::take(&mut *self.gc.borrow_mut());
        let Some(hooks) = gc.hooks else {
            return;
        };
        if gc.heap.is_null() {
            return;
        }
        if let Some(begin) = hooks.teardown_begin {
            // SAFETY: the heap is live until `teardown_end` returns.
            unsafe { begin(gc.heap) };
        }
        for root in &gc.roots {
            if root.flags & GC_ROOT_SURVIVES_TEARDOWN != 0 {
                // SAFETY: registered root functions stay valid for the realm's life.
                unsafe { (root.roots)(gc.heap, root.context, GC_PHASE_TEARDOWN) };
            }
        }
        if let Some(end) = hooks.teardown_end {
            // SAFETY: the last use of the heap; the prelude frees it here.
            unsafe { end(gc.heap) };
        }
        // The heap is gone: no answer may name it any more.
        drop(current);
        for root in &gc.roots {
            // SAFETY: the reset phase receives no heap and touches only unit statics.
            unsafe { (root.roots)(std::ptr::null_mut(), root.context, GC_PHASE_RESET) };
        }
    }
}

impl Drop for CallableStore {
    fn drop(&mut self) {
        // SAFETY: publication transfers this pointer to the realm together with its matching
        // destructor. A CallableStore is inserted at most once and dropped exactly once.
        unsafe { (self.drop)(self.pointer) };
    }
}

impl NativeRealm {
    fn pin(&self, reason: u32) {
        self.affinity.set(self.affinity.get() | reason);
    }

    fn new() -> Result<Self, kiln_runtime::RuntimeBuildError> {
        Self::with_reactor(kiln_runtime::Reactor::current_thread()?)
    }

    fn on_reactor(
        reactor: Arc<kiln_runtime::Reactor>,
    ) -> Result<Self, kiln_runtime::RuntimeBuildError> {
        Self::with_reactor(reactor)
    }

    fn with_reactor(
        reactor: Arc<kiln_runtime::Reactor>,
    ) -> Result<Self, kiln_runtime::RuntimeBuildError> {
        Ok(Self {
            runtime: NativeRuntime {
                executor: RefCell::new(Some(Executor::on_reactor(Arc::clone(&reactor)))),
                reactor,
                state: RefCell::new(RealmState::new(
                    TokioHost::new(),
                    RealmCapabilities::default(),
                )),
            },
            work: RefCell::new(Realm::new()),
            continuations: RefCell::new(HashMap::new()),
            #[cfg(feature = "realm-http")]
            http_connections: RefCell::new(HashMap::new()),
            #[cfg(feature = "realm-http")]
            next_http_client_request_id: Cell::new(1),
            cancelled: Rc::new(RefCell::new(HashSet::new())),
            queued: Rc::new(RefCell::new(HashSet::new())),
            symbols: RefCell::new(hashbrown::HashMap::with_hasher(
                std::collections::hash_map::RandomState::new(),
            )),
            symbol_keys: RefCell::new(HashMap::new()),
            callable_stores: RefCell::new(HashMap::new()),
            value_slots: RefCell::new(HashMap::new()),
            argv: RefCell::new(Vec::new()),
            entropy: Cell::new(None),
            entered: Cell::new(false),
            diagnostic: RefCell::new(RealmDiagnostic::default()),
            drive_context: Cell::new((std::ptr::null_mut(), std::ptr::null_mut())),
            clock: Cell::new(NativeClock::Virtual),
            promise_lane: Cell::new(None),
            introspect_armed: Cell::new(false),
            scheduled_wait: RefCell::new(None),
            affinity: Cell::new(0),
            worker_scope: Rc::new(RefCell::new(
                kiln_runtime::workers::RealmScopeState::capture_current(),
            )),
            #[cfg(feature = "ffi")]
            ffi: RefCell::new(crate::ffi::State::default()),
            gc: RefCell::new(GcState::default()),
        })
    }
}

struct NativeRuntime {
    reactor: Arc<kiln_runtime::Reactor>,
    executor: RefCell<Option<Executor>>,
    state: RefCell<RealmState<TokioHost>>,
}

impl NativeRuntime {
    fn executor(&self) -> std::cell::Ref<'_, Executor> {
        std::cell::Ref::map(self.executor.borrow(), |executor| {
            executor
                .as_ref()
                .expect("realm executor is installed on this lane")
        })
    }

    /// Drop the empty task set on its old lane before the realm record crosses threads.
    fn detach_executor(&self) {
        let executor = self
            .executor
            .borrow_mut()
            .take()
            .expect("executor was attached");
        drop(executor);
    }

    fn attach_executor(&self) {
        let previous = self
            .executor
            .replace(Some(Executor::on_reactor(Arc::clone(&self.reactor))));
        assert!(
            previous.is_none(),
            "realm was detached before a lane installs its executor"
        );
    }

    fn state_mut(&self) -> Option<std::cell::RefMut<'_, RealmState<TokioHost>>> {
        self.state.try_borrow_mut().ok()
    }
}

impl Drop for NativeRuntime {
    fn drop(&mut self) {
        let executor = self
            .executor
            .get_mut()
            .take()
            .unwrap_or_else(|| Executor::on_reactor(Arc::clone(&self.reactor)));
        executor.shut_down(self.state.get_mut());
    }
}

struct ActiveRealm {
    realm: Rc<NativeRealm>,
    worker_scope: Option<kiln_runtime::workers::RealmScopeGuard>,
    /// Generated code runs only inside an active realm, so the thread may read shared cache
    /// state for exactly as long as this lives (`reclaim`). Dropped after `Drop::drop`.
    _online: reclaim::Online,
}

impl ActiveRealm {
    fn enter(handle: u64, realm: Rc<NativeRealm>) -> Option<Self> {
        // Entering the realm is a turn boundary for a wall-clock realm (ADR 0044).
        tick_clock(&realm);
        if realm.entered.replace(true) {
            return None;
        }
        let activated = ACTIVE_REALM.with(|active| {
            if active.get() != 0 {
                false
            } else {
                active.set(handle);
                true
            }
        });
        if !activated {
            realm.entered.set(false);
            return None;
        }
        // Every private ABI entry asks for the active realm; a server asks dozens of times
        // per request. Keep the handle's realm and its collector heap where a read costs a
        // thread-local load rather than a hash lookup.
        ACTIVE_REALM_RC.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&realm)));
        ACTIVE_HEAP.with(|slot| slot.set(realm.gc.borrow().heap));
        bump_generation();
        // A scheduler lane hosts several realms. Its thread-local worker protocol and signal
        // state must describe only the realm holding this turn's exclusive lease, including a
        // worker once its graph runner advances by turns.
        let worker_scope = current_scheduler()
            .map(|_| kiln_runtime::workers::RealmScopeGuard::enter(Rc::clone(&realm.worker_scope)));
        Some(Self {
            realm,
            worker_scope,
            _online: reclaim::online(),
        })
    }
}

impl Drop for ActiveRealm {
    fn drop(&mut self) {
        self.worker_scope.take();
        ACTIVE_REALM.with(|active| active.set(0));
        ACTIVE_REALM_RC.with(|slot| slot.borrow_mut().take());
        ACTIVE_HEAP.with(|slot| slot.set(std::ptr::null_mut()));
        bump_generation();
        self.realm.entered.set(false);
    }
}

static NEXT_REALM: AtomicU64 = AtomicU64::new(1);
static NEXT_SYMBOL: AtomicU64 = AtomicU64::new(1);

std::thread_local! {
    static REALMS: RefCell<HashMap<u64, Rc<NativeRealm>>> = RefCell::new(HashMap::new());
    static ACTIVE_REALM: Cell<u64> = const { Cell::new(0) };
    static ACTIVE_REALM_RC: RefCell<Option<Rc<NativeRealm>>> = const { RefCell::new(None) };
    static ACTIVE_HEAP: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
    /// Bumped whenever the answer to "which realm, which heap" changes on this thread: a
    /// realm entered or left, a heap attached. The prelude caches per-realm answers in
    /// thread-locals of its own and compares against this before trusting one, reading it
    /// through the pointer `kiln_realm_generation_slot` hands out, which is stable for the
    /// thread's life. A generation rather than the heap pointer itself, because a heap
    /// address can be freed and reused by the next realm on the same thread.
    static ACTIVE_GENERATION: Cell<u64> = const { Cell::new(1) };
    /// Set only while a scheduler lane constructs a realm, so `kiln_realm_create` puts its
    /// LocalSet on that lane while all lanes share the scheduler's reactor.
    static SCHEDULED_REACTOR: RefCell<Option<Arc<kiln_runtime::Reactor>>> = const { RefCell::new(None) };
    /// Set only across a scheduled C step. A nested provider drive uses this ticket to park
    /// without exposing the scheduler's Rust types through the private C ABI.
    static SCHEDULED_WAKER: RefCell<Option<Waker>> = const { RefCell::new(None) };
    /// The same lane pool that runs this generated turn also accepts parallel chunks it calls.
    static SCHEDULED_POOL: RefCell<Option<Weak<PinnedTurnScheduler>>> = const { RefCell::new(None) };
}

pub(crate) fn current_scheduler() -> Option<Arc<PinnedTurnScheduler>> {
    SCHEDULED_POOL.with(|slot| slot.borrow().as_ref().and_then(Weak::upgrade))
}

/// Temporarily replace a scheduled lane before a synchronous foreign wait occupies it.
/// A dedicated-thread realm has no scheduler lane to replace.
pub(crate) fn enter_blocking_scheduler()
-> std::io::Result<Option<kiln_runtime::turn_scheduler::BlockingTurnGuard>> {
    match current_scheduler() {
        Some(scheduler) if !scheduler.is_deterministic() => scheduler.enter_blocking().map(Some),
        _ => Ok(None),
    }
}

pub(crate) struct ScheduledPoolGuard(Option<Weak<PinnedTurnScheduler>>);

impl ScheduledPoolGuard {
    pub(crate) fn enter(pool: Option<Weak<PinnedTurnScheduler>>) -> Self {
        Self(SCHEDULED_POOL.with(|slot| slot.replace(pool)))
    }
}

impl Drop for ScheduledPoolGuard {
    fn drop(&mut self) {
        SCHEDULED_POOL.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

pub(crate) struct ScheduledReactorGuard(Option<Arc<kiln_runtime::Reactor>>);

impl ScheduledReactorGuard {
    pub(crate) fn enter(reactor: Arc<kiln_runtime::Reactor>) -> Self {
        Self(SCHEDULED_REACTOR.with(|slot| slot.replace(Some(reactor))))
    }
}

impl Drop for ScheduledReactorGuard {
    fn drop(&mut self) {
        SCHEDULED_REACTOR.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

pub(crate) struct ScheduledWakerGuard(Option<Waker>);

impl ScheduledWakerGuard {
    pub(crate) fn enter(waker: Waker) -> Self {
        Self(SCHEDULED_WAKER.with(|slot| slot.replace(Some(waker))))
    }
}

impl Drop for ScheduledWakerGuard {
    fn drop(&mut self) {
        SCHEDULED_WAKER.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

fn bump_generation() {
    ACTIVE_GENERATION.with(|generation| generation.set(generation.get().wrapping_add(1)));
}

/// The address of this thread's realm generation, for the prelude to read directly.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_generation_slot() -> *const u64 {
    ACTIVE_GENERATION.with(|generation| generation.as_ptr() as *const u64)
}

fn ffi(operation: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(RUNTIME_ERROR)
}

/// Whether the engine's `KILN_HTTP_TRACE` log is on. Without `realm-http` there is no
/// engine, so the trace is always off; the call sites stay shared.
#[cfg(feature = "realm-http")]
fn http_trace_enabled() -> bool {
    kiln_runtime::http_engine::trace_enabled()
}

#[cfg(not(feature = "realm-http"))]
fn http_trace_enabled() -> bool {
    false
}

fn realm(handle: u64) -> Option<Rc<NativeRealm>> {
    if handle == 0 {
        return None;
    }
    REALMS.with(|realms| realms.borrow().get(&handle).cloned())
}

/// Exclusive ownership of a realm between language turns. Its constructor removes the only
/// strong realm reference from the source lane's registry and drops that lane's empty LocalSet.
/// A destination lane installs a fresh LocalSet before it makes the realm visible again.
struct DetachedRealm(Rc<NativeRealm>);

// SAFETY: `detach_realm` is the sole constructor. It checks there is exactly one strong owner,
// no active drive, no local waiter and no operation that could publish a thread-affine callback.
// The executor's LocalSet is destroyed on the old lane before this crosses a thread. The
// remaining `Rc`/`RefCell` graph has no concurrent observer; only `attach_realm` may publish it
// again, into one destination lane's thread-local registry.
unsafe impl Send for DetachedRealm {}

fn reset_lane_frame_roots(realm: &NativeRealm) {
    // Generated module callbacks delegate LANE_RESET to the C prelude's frame-register reset. Those
    // registers are TLS rather than realm fields: neither the source lane nor the destination
    // lane may retain a prior realm's value after an exclusive lease changes lanes.
    let gc = realm.gc.borrow();
    for root in &gc.roots {
        // SAFETY: LANE_RESET has no heap argument and the registered callback and context remain
        // valid for this realm's life. Unlike teardown RESET, it preserves fallback key owners.
        unsafe { (root.roots)(ptr::null_mut(), root.context, GC_PHASE_LANE_RESET) };
    }
}

fn detach_realm(handle: u64) -> Option<DetachedRealm> {
    REALMS.with(|registry| {
        let mut registry = registry.borrow_mut();
        let realm = registry.get(&handle)?;
        if realm.entered.get()
            || realm.affinity.get() != 0
            || realm.scheduled_wait.borrow().is_some()
            || !matches!(realm.clock.get(), NativeClock::Wall { .. })
            || Rc::strong_count(realm) != 1
        {
            return None;
        }
        let realm = registry.remove(&handle).expect("realm was inspected above");
        reset_lane_frame_roots(&realm);
        realm.runtime.detach_executor();
        Some(DetachedRealm(realm))
    })
}

fn attach_realm(handle: u64, detached: DetachedRealm) {
    let DetachedRealm(realm) = detached;
    realm.runtime.attach_executor();
    reset_lane_frame_roots(&realm);
    REALMS.with(|registry| {
        let old = registry.borrow_mut().insert(handle, realm);
        assert!(old.is_none(), "a detached realm has one registry owner");
    });
}

fn active_realm() -> Option<Rc<NativeRealm>> {
    ACTIVE_REALM_RC.with(|slot| slot.borrow().clone())
}

/// Wake the active realm's drive if it is parked: a callback run inside a connection task has
/// asked the realm to end (`process.exit` in a worker or a thread-mode realm), and the drive
/// must see that now rather than at its next timeout (realms issue 23).
pub(crate) fn wake_active_drive() {
    if let Some(realm) = active_realm()
        && let Ok(runtime) = realm.runtime.state.try_borrow()
    {
        runtime.wake_drive();
    }
}

/// The active realm's `node:ffi` state, for `ffi.rs`; `None` outside a realm.
#[cfg(feature = "ffi")]
pub(crate) fn with_active_ffi_state<R>(
    operation: impl FnOnce(&RefCell<crate::ffi::State>) -> R,
) -> Option<R> {
    active_realm().map(|realm| {
        realm.pin(AFFINITY_FFI);
        operation(&realm.ffi)
    })
}

/// Mark the current realm as permanently bound before a synchronous provider operation can
/// retain or block on a thread-affine pointer.
pub(crate) fn pin_active_realm(reason: u32) {
    if let Some(realm) = active_realm() {
        realm.pin(reason);
    }
}

fn active_handle(handle: u64) -> Option<Rc<NativeRealm>> {
    ACTIVE_REALM.with(|active| {
        if active.get() == handle {
            realm(handle)
        } else {
            None
        }
    })
}

unsafe fn borrowed_bytes<'a>(pointer: *const u8, length: u64) -> Option<&'a [u8]> {
    let length = usize::try_from(length).ok()?;
    if length == 0 {
        return Some(&[]);
    }
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the FFI caller promises this byte span remains readable for the submission
    // call; every operation copies it before returning.
    Some(unsafe { std::slice::from_raw_parts(pointer, length) })
}

unsafe fn host_operation(view: &HostOpView) -> Option<AsyncHostOp> {
    // SAFETY: the caller keeps both framed spans live for this call.
    let text = unsafe { borrowed_bytes(view.text, view.text_len) }?;
    // SAFETY: the caller keeps both framed spans live for this call.
    let bytes = unsafe { borrowed_bytes(view.bytes, view.bytes_len) }?;
    if view.operation == OP_PROCESS_RUN {
        let framed = kiln_runtime::decode_process_wire(bytes).ok()?;
        return Some(AsyncHostOp::ProcessRun {
            command: String::from_utf8(framed.command).ok()?,
            args: framed
                .args
                .into_iter()
                .map(String::from_utf8)
                .collect::<Result<_, _>>()
                .ok()?,
            stdin: framed.stdin,
        });
    }
    let text = std::str::from_utf8(text).ok()?.to_owned();
    Some(match view.operation {
        OP_READ_FILE => AsyncHostOp::ReadFile { path: text },
        OP_WRITE_FILE => AsyncHostOp::WriteFile {
            path: text,
            data: bytes.to_vec(),
        },
        OP_STAT => AsyncHostOp::Stat { path: text },
        OP_CLOSE_RESOURCE => AsyncHostOp::CloseResource {
            resource: ResourceId::new(view.resource),
        },
        OP_SET_RESOURCE_REFERENCED => AsyncHostOp::SetResourceReferenced {
            resource: ResourceId::new(view.resource),
            referenced: view.argument != 0,
        },
        OP_RESOURCE_STATE => AsyncHostOp::ResourceState {
            resource: ResourceId::new(view.resource),
        },
        OP_TCP_LISTEN => AsyncHostOp::TcpListen {
            host: text,
            port: u16::try_from(view.argument).ok()?,
        },
        OP_TCP_ACCEPT => AsyncHostOp::TcpAccept {
            listener: ResourceId::new(view.resource),
        },
        OP_TCP_CONNECT => AsyncHostOp::TcpConnect {
            host: text,
            port: u16::try_from(view.argument).ok()?,
        },
        OP_TCP_READ => AsyncHostOp::TcpRead {
            stream: ResourceId::new(view.resource),
            max_bytes: usize::try_from(view.argument).ok()?,
        },
        OP_TCP_WRITE => AsyncHostOp::TcpWrite {
            stream: ResourceId::new(view.resource),
            data: bytes.to_vec(),
        },
        OP_TCP_SHUTDOWN => AsyncHostOp::TcpShutdown {
            stream: ResourceId::new(view.resource),
        },
        OP_STDIN_READ => AsyncHostOp::StdinRead {
            max_bytes: usize::try_from(view.argument).ok()?,
        },
        OP_MKDIR => AsyncHostOp::Mkdir {
            path: text,
            recursive: view.argument != 0,
        },
        OP_PORT_WAIT => AsyncHostOp::PortWait {
            port: view.resource,
        },
        _ => return None,
    })
}

#[cfg(feature = "realm-http")]
unsafe fn parse_headers(pointer: *const u8, length: u64) -> Option<Vec<(String, String)>> {
    // SAFETY: callers keep this private ABI span live until the synchronous command returns.
    let bytes = unsafe { borrowed_bytes(pointer, length) }?;
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut headers = Vec::new();
    while let Some(name) = fields.next() {
        if name.is_empty() && fields.next().is_none() {
            break;
        }
        let value = fields.next()?;
        headers.push((
            std::str::from_utf8(name).ok()?.to_owned(),
            std::str::from_utf8(value).ok()?.to_owned(),
        ));
    }
    Some(headers)
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_server_close_active(resource: u64, mode: i32) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        let mode = match mode {
            HTTP_CLOSE_GRACEFUL => HttpCloseMode::Graceful,
            HTTP_CLOSE_ABORT => HttpCloseMode::Abort,
            _ => return INVALID_ARGUMENT,
        };
        // A missing registration is a failed capability lookup, never permission to treat
        // the socket as a raw resource: only the engine that owns the transport can close
        // it, for either role. `Closed` reaches the handler from the engine's task.
        if runtime.http_close(kiln_host_abi::ResourceId::new(resource), mode) {
            0
        } else {
            INVALID_ARGUMENT
        }
    })
}

fn drive(realm: &NativeRealm, host: *mut c_void, thrown: *mut c_void) -> i32 {
    // One `block_on` for the life of the drive (ADR 0040 step 4, ADR 0046). Every callback
    // the language makes back into this archive is already in context, and the HTTP engine's
    // tasks call the language while this future is parked, so no RefCell of the realm may be
    // held across an await here.
    realm.drive_context.set((host, thrown));
    let status = realm.runtime.executor().block_on(drive_async(realm));
    realm
        .drive_context
        .set((std::ptr::null_mut(), std::ptr::null_mut()));
    status
}

/// Poll exactly one language turn on a scheduler lane. A local task waits on the host signal
/// and wakes the realm's ticket; the lane itself is free to run other realms while this one is
/// parked. The caller enters `ActiveRealm` and keeps the generated host/result slots alive.
fn drive_scheduled(
    realm: &NativeRealm,
    host: *mut c_void,
    thrown: *mut c_void,
    waker: &Waker,
) -> ScheduledStep {
    realm.drive_context.set((host, thrown));
    let step = drive_scheduled_inner(realm, waker);
    realm
        .drive_context
        .set((std::ptr::null_mut(), std::ptr::null_mut()));
    step
}

enum ScheduledStep {
    Ready,
    Parked,
    ParkedUntil(std::time::Instant),
    Done(i32),
}

fn drive_scheduled_inner(realm: &NativeRealm, waker: &Waker) -> ScheduledStep {
    // The LocalSet runs only on this realm's lane. Its waker is the ticket, so a host task
    // becoming ready while another realm runs queues this realm without a parked OS thread.
    realm.runtime.executor().poll_local_once(waker);
    let pending_wait = { realm.scheduled_wait.borrow_mut().take() };
    if let Some(mut wait) = pending_wait {
        if wait.wake.get().is_none()
            && !wait.advanced
            && wait.timer_armed.get()
            && let Some(delay) = wait.virtual_delay
        {
            // The waiter has polled its timer after the preceding yield. Advancing earlier
            // makes the timer start at the new time and leaves the virtual realm parked.
            wait.advanced = true;
            // Tokio's timer wheel rounds to its next millisecond tick. Advancing by exactly
            // the requested delay can stop just before that tick and leave the waiter asleep.
            // This cushion affects only Tokio's paused clock; the language clock still runs
            // the timer at its declared deadline.
            realm
                .runtime
                .executor()
                .advance_virtual_time(delay.saturating_add(Duration::from_millis(1)));
            realm.runtime.executor().poll_local_once(waker);
        }
        let Some(wake) = wait.wake.get() else {
            let virtual_clock = matches!(realm.clock.get(), NativeClock::Virtual);
            realm.scheduled_wait.replace(Some(wait));
            if virtual_clock {
                // A current-thread reactor has no background I/O driver. Give parked sockets
                // another bounded tick without pinning an OS thread per realm.
                return ScheduledStep::ParkedUntil(Instant::now() + Duration::from_millis(1));
            }
            return ScheduledStep::Parked;
        };
        if wait.yield_first
            && wake == kiln_runtime::DriveWake::Timer
            && matches!(realm.clock.get(), NativeClock::Virtual)
        {
            let status = run_one_timer(realm);
            if status != 0 {
                return ScheduledStep::Done(status);
            }
        }
    }
    let step = realm
        .runtime
        .executor()
        .scope(|| drive_one_ready_turn(realm));
    match step {
        DriveStep::Ready => ScheduledStep::Ready,
        DriveStep::Done(status) => ScheduledStep::Done(status),
        DriveStep::Parked {
            signal,
            delay,
            yield_first,
        } => {
            let delay = introspect_park(realm, delay);
            // With no host operation to wake us, a wall-clock timer belongs to the lane's
            // deadline heap. It needs no Tokio task, even when thousands of realms are idle.
            if !yield_first
                && matches!(realm.clock.get(), NativeClock::Wall { .. })
                && let Some(delay) = delay
            {
                return ScheduledStep::ParkedUntil(std::time::Instant::now() + delay);
            }
            let wake = Rc::new(Cell::new(None));
            let wake_for_task = Rc::clone(&wake);
            let virtual_delay = matches!(realm.clock.get(), NativeClock::Virtual)
                .then_some(delay)
                .flatten();
            let timer_armed = Rc::new(Cell::new(false));
            let armed_for_task = Rc::clone(&timer_armed);
            let ticket = waker.clone();
            realm.pin(AFFINITY_LOCAL_TASK);
            realm.runtime.executor().spawn_local(async move {
                if yield_first {
                    kiln_runtime::yield_to_tasks().await;
                }
                let mut waiting = Box::pin(kiln_runtime::wait_for_turn(signal, delay));
                let result = std::future::poll_fn(|cx| {
                    let progress = waiting.as_mut().poll(cx);
                    if progress.is_pending() {
                        armed_for_task.set(true);
                    }
                    progress
                })
                .await;
                wake_for_task.set(Some(result));
                ticket.wake();
            });
            realm.scheduled_wait.replace(Some(ScheduledWait {
                wake: Rc::clone(&wake),
                yield_first,
                timer_armed,
                virtual_delay,
                advanced: false,
            }));
            realm.runtime.executor().poll_local_once(waker);
            if virtual_delay.is_some() {
                // One successor turn observes whether the waiter has armed its timer before
                // advancing virtual time. The ticket coalesces with any task wake meanwhile.
                ScheduledStep::Ready
            } else if matches!(realm.clock.get(), NativeClock::Virtual) && wake.get().is_none() {
                ScheduledStep::ParkedUntil(Instant::now() + Duration::from_millis(1))
            } else {
                ScheduledStep::Parked
            }
        }
    }
}

async fn drive_async(realm: &NativeRealm) -> i32 {
    loop {
        match drive_one_ready_turn(realm) {
            DriveStep::Ready => continue,
            DriveStep::Done(status) => return status,
            DriveStep::Parked {
                signal,
                delay,
                yield_first,
            } => {
                if yield_first {
                    // A task woken by the previous turn gets to publish its completion before
                    // this realm parks. This was the order of the original continuous drive.
                    kiln_runtime::yield_to_tasks().await;
                }
                let delay = introspect_park(realm, delay);
                let parked = reclaim::park();
                let wake = kiln_runtime::wait_for_turn(signal, delay).await;
                drop(parked);
                if yield_first
                    && wake == kiln_runtime::DriveWake::Timer
                    && matches!(realm.clock.get(), NativeClock::Virtual)
                {
                    let status = run_one_timer(realm);
                    if status != 0 {
                        return status;
                    }
                }
            }
        }
    }
}

/// One synchronous language turn, ending before the executor waits. The scheduler can later
/// lease a realm for exactly this call; the continuous drive keeps the existing wait policy
/// around it while the launcher and executor are split into schedulable pieces.
enum DriveStep {
    Ready,
    Parked {
        signal: kiln_runtime::DriveSignal,
        delay: Option<std::time::Duration>,
        yield_first: bool,
    },
    Done(i32),
}

fn run_one_timer(realm: &NativeRealm) -> i32 {
    let timer = take_next_timer(realm);
    if let Some(WorkItem::Timer { work, .. }) = timer {
        let (host, thrown) = realm.drive_context.get();
        work.run(host, thrown)
    } else {
        0
    }
}

fn drive_one_ready_turn(realm: &NativeRealm) -> DriveStep {
    // A turn boundary: no generated frame is on this thread's stack.
    reclaim::quiescent();
    // A worker whose parent called `terminate()` ends here (realms issue 06); the launcher
    // reads the exit it asked for.
    if kiln_runtime::workers::terminate_requested() {
        kiln_runtime::workers::request_exit(1);
        return DriveStep::Done(WORKER_EXIT);
    }
    // So does one whose `process.exit` a callback's caller did not unwind with (a close
    // callback's status is not every dispatcher's to return): the realm is over the moment
    // it asked, not when its loop next runs dry (realms issue 23).
    if kiln_runtime::workers::exit_requested() {
        return DriveStep::Done(WORKER_EXIT);
    }
    tick_clock(realm);
    collect_if_due(realm);
    introspect_if_requested(realm);
    let status = drain_realm_queues(realm);
    if status != 0 {
        return DriveStep::Done(status);
    }
    if matches!(realm.clock.get(), NativeClock::Virtual) {
        realm.runtime.executor().pause_time();
    }
    let turn = realm
        .runtime
        .state
        .borrow_mut()
        .ready_turn::<NativeMicrotask, NativeWork>();
    match turn {
        Some(RuntimeTurn::Completion(completion)) => {
            if let Some(status) = route_completion(realm, completion) {
                return DriveStep::Done(status);
            }
            while let Some(RuntimeTurn::Completion(next)) = {
                let mut runtime = realm.runtime.state.borrow_mut();
                runtime.ready_turn::<NativeMicrotask, NativeWork>()
            } {
                if let Some(status) = route_completion(realm, next) {
                    return DriveStep::Done(status);
                }
            }
            return DriveStep::Ready;
        }
        Some(RuntimeTurn::Failure(_)) => return DriveStep::Done(RUNTIME_ERROR),
        Some(RuntimeTurn::Timer(WorkItem::Timer { work, .. })) => {
            let (host, thrown) = realm.drive_context.get();
            let status = work.run(host, thrown);
            if status != 0 {
                return DriveStep::Done(status);
            }
            return DriveStep::Ready;
        }
        Some(RuntimeTurn::Timer(WorkItem::Microtask(_))) => {
            unreachable!("the native drive drains microtasks before selecting a turn")
        }
        // The native dispatch calls the language directly; nothing is ever queued for
        // this drive, so a ready turn never reports it.
        Some(RuntimeTurn::Protocol) | None => {}
    }
    let (referenced, pending, in_flight, deadline, now, signal) = {
        let runtime = realm.runtime.state.borrow();
        let mut work = realm.work.borrow_mut();
        (
            runtime.has_referenced_host_work(),
            runtime.has_pending_host_work(),
            runtime.requests_in_flight(),
            work.next_timer_deadline(),
            work.now(),
            runtime.completion_signal(),
        )
    };
    if http_trace_enabled() {
        eprintln!(
            "[kiln-drive] referenced={referenced} pending={pending} in_flight={in_flight} deadline={deadline:?} host={}",
            realm.runtime.state.borrow().describe_host_work(),
        );
    }
    // A timer that is already due runs now. It is not a wait case: under load the
    // completion signal always has a permit, and a select that prefers it would let a
    // due timer wait forever (measured: setTimeout(0) at 13 ms median on a saturated
    // server while every other route took 0.13 ms).
    if let Some(due) = deadline
        && due <= now
    {
        let status = run_one_timer(realm);
        return if status == 0 {
            DriveStep::Ready
        } else {
            DriveStep::Done(status)
        };
    }
    // A completion can arrive only while an operation is pending, and it matters only
    // while something referenced keeps the realm alive; a direct connection with a
    // response outstanding is alive on its own. Otherwise only virtual timers remain.
    let completions_possible = referenced && pending;
    if !completions_possible && in_flight == 0 {
        let Some(deadline) = deadline else {
            return DriveStep::Done(0);
        };
        // A wall-clock realm (ADR 0044) waits the real interval; the next turn finds the
        // timer due. A virtual realm jumps its clock to the deadline instead.
        if matches!(realm.clock.get(), NativeClock::Wall { .. }) {
            let Some(signal) = signal else {
                return DriveStep::Done(RUNTIME_ERROR);
            };
            return DriveStep::Parked {
                signal,
                delay: Some(deadline.saturating_sub(now)),
                yield_first: false,
            };
        }
        let status = run_one_timer(realm);
        return if status == 0 {
            DriveStep::Ready
        } else {
            DriveStep::Done(status)
        };
    }
    let Some(signal) = signal else {
        return DriveStep::Done(RUNTIME_ERROR);
    };
    // A wall-clock timer wake may be an introspection poll rather than the deadline. A
    // virtual timer wake consumes one timer after the executor returns from its wait.
    DriveStep::Parked {
        signal,
        delay: deadline.map(|deadline| deadline.saturating_sub(now)),
        yield_first: true,
    }
}

/// How many timers ahead of the one about to run the drive starts bringing a timer's payload
/// into the cache. A hundred thousand pending tasks each wake a promise, and through it a
/// suspended frame, that were last touched a whole wave of timers ago; a hint four runs
/// early lets the miss overlap the runs in between.
const TIMER_PREFETCH_AHEAD: usize = 4;

/// Takes the next timer to run, and hints the one `TIMER_PREFETCH_AHEAD` behind it.
fn take_next_timer(realm: &NativeRealm) -> Option<WorkItem<NativeMicrotask, NativeWork>> {
    let mut work = realm.work.borrow_mut();
    let timer = work.run_next_timer();
    if let Some(upcoming) = work.upcoming_timer_work(TIMER_PREFETCH_AHEAD) {
        upcoming.prefetch();
    }
    timer
}

/// The longest a wall-clock realm parks while a `SIGUSR1` handler is armed.
///
/// Without a cap an idle server parks on its completion signal until the next connection
/// arrives, so it would answer a signal only once it stopped being idle -- which inverts the
/// feature, since the process most worth interrogating is the one that has gone quiet. Five
/// wakeups a second is what that costs a process that is by hypothesis doing nothing.
const INTROSPECT_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Caps a park so the turn boundary comes round often enough to notice a `SIGUSR1`.
///
/// A virtual-clock realm is never capped. Its park costs no wall time by construction (ADR
/// 0012, ADR 0028) and capping it would make deterministic execution depend on real time,
/// which would surface as flake in the corpora rather than as a wrong number here.
fn introspect_park(
    realm: &NativeRealm,
    delay: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    if !realm.introspect_armed.get() || !matches!(realm.clock.get(), NativeClock::Wall { .. }) {
        return delay;
    }
    Some(delay.map_or(INTROSPECT_POLL, |delay| delay.min(INTROSPECT_POLL)))
}

/// Route one host completion to its continuation, which runs as a microtask. `Some` is a
/// failure status.
fn route_completion(realm: &NativeRealm, completion: kiln_host_abi::Completion) -> Option<i32> {
    let continuation = realm.continuations.borrow_mut().remove(&completion.request);
    let continuation = continuation?;
    let payload =
        CompletionPayload::from_result(completion.request, completion.sequence, completion.result);
    let completion_work = Box::new(CompletionWork {
        request: completion.request,
        continuation: Some(continuation),
        payload,
        cancelled: Rc::clone(&realm.cancelled),
        queued: Rc::clone(&realm.queued),
    });
    realm.queued.borrow_mut().insert(completion.request);
    let pointer = Box::into_raw(completion_work).cast::<c_void>();
    let Some(work) = NativeMicrotask::with_roots(
        pointer,
        Some(run_completion),
        Some(drop_completion),
        WorkRoots::Completion,
    ) else {
        // SAFETY: `pointer` was produced by Box::into_raw and ownership has not been
        // transferred because the callback pair was rejected.
        unsafe { drop_completion(pointer) };
        return Some(RUNTIME_ERROR);
    };
    realm.work.borrow_mut().queue_microtask(work);
    None
}

/// Creates one current-thread Realm Runtime and writes its opaque identity to `out`.
///
/// # Safety
///
/// `out` must be a writable `u64` slot owned by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_create(out: *mut u64) -> i32 {
    ffi(|| {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        let reactor = SCHEDULED_REACTOR.with(|slot| slot.borrow().clone());
        let created = match reactor {
            Some(reactor) => NativeRealm::on_reactor(reactor),
            None => NativeRealm::new(),
        };
        let Ok(realm) = created else {
            return RUNTIME_ERROR;
        };
        let Ok(handle) = NEXT_REALM.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        }) else {
            return RUNTIME_ERROR;
        };
        REALMS.with(|realms| {
            realms.borrow_mut().insert(handle, Rc::new(realm));
        });
        // SAFETY: the caller supplies a writable slot and null was rejected above.
        unsafe { out.write(handle) };
        0
    })
}

/// Executes `entry(context)` as the current task of the owning Realm Runtime.
///
/// Recursive entry fails closed instead of attempting a nested `block_on`.
pub unsafe extern "C" fn enter(handle: u64, entry: Option<Entry>, context: *mut c_void) -> i32 {
    ffi(|| {
        let Some(entry) = entry else {
            return INVALID_ARGUMENT;
        };
        let Some(realm) = realm(handle) else {
            return INVALID_ARGUMENT;
        };
        let Some(_active) = ActiveRealm::enter(handle, Rc::clone(&realm)) else {
            return INVALID_ARGUMENT;
        };
        if realm.runtime.state.borrow().is_shutdown() {
            return RUNTIME_ERROR;
        }
        realm
            .runtime
            .executor()
            .block_on(async move { unsafe { entry(context) } })
    })
}

/// C-exported spelling of [`enter`].
///
/// # Safety
///
/// `entry`, when present, must be a valid C function and `context` must satisfy that
/// function's contract for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_enter(
    handle: u64,
    entry: Option<Entry>,
    context: *mut c_void,
) -> i32 {
    // SAFETY: this function has exactly the same caller contract as `enter`.
    unsafe { enter(handle, entry, context) }
}

/// Returns one when generated code is already executing inside a Realm.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_is_active() -> i32 {
    i32::from(ACTIVE_REALM.with(|active| active.get() != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_diagnostic_reset() -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        *realm.diagnostic.borrow_mut() = RealmDiagnostic::default();
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_diagnostic_note(
    site: *const u8,
    site_len: u64,
    instruction: u64,
    operation: *const u8,
    operation_len: u64,
    depth: u64,
    detail: *const u8,
    detail_len: u64,
    function: *const u8,
    function_len: u64,
    method: u64,
    receiver_tag: u64,
    has_method: i32,
) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(site) = (unsafe { borrowed_bytes(site, site_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(operation) = (unsafe { borrowed_bytes(operation, operation_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(detail) = (unsafe { borrowed_bytes(detail, detail_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(function) = (unsafe { borrowed_bytes(function, function_len) }) else {
            return INVALID_ARGUMENT;
        };
        let mut diagnostic = realm.diagnostic.borrow_mut();
        if !diagnostic.site.is_empty() && depth <= diagnostic.depth {
            return 0;
        }
        diagnostic.site = site.to_vec();
        diagnostic.operation = operation.to_vec();
        diagnostic.detail = detail.to_vec();
        diagnostic.function = function.to_vec();
        diagnostic.instruction = instruction;
        diagnostic.depth = depth;
        diagnostic.method = method;
        diagnostic.receiver_tag = receiver_tag;
        diagnostic.has_method = has_method != 0;
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_diagnostic_render(
    out: *mut u8,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        if out_len.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let diagnostic = realm.diagnostic.borrow();
        let mut rendered = if diagnostic.site.is_empty() {
            b"unknown native operation".to_vec()
        } else {
            format!(
                "module={} instruction={}",
                String::from_utf8_lossy(&diagnostic.site),
                diagnostic.instruction
            )
            .into_bytes()
        };
        for (label, value) in [
            (" operation=", &diagnostic.operation),
            (" detail=", &diagnostic.detail),
            (" function=", &diagnostic.function),
        ] {
            if !value.is_empty() {
                rendered.extend_from_slice(label.as_bytes());
                rendered.extend_from_slice(value);
            }
        }
        if diagnostic.has_method {
            rendered.extend_from_slice(
                format!(
                    " method={} receiver_tag={}",
                    diagnostic.method, diagnostic.receiver_tag
                )
                .as_bytes(),
            );
        } else if diagnostic.receiver_tag != 0 {
            rendered
                .extend_from_slice(format!(" receiver_tag={}", diagnostic.receiver_tag).as_bytes());
        }
        unsafe { out_len.write(rendered.len() as u64) };
        if capacity < rendered.len() as u64 {
            return INVALID_ARGUMENT;
        }
        if !rendered.is_empty() {
            if out.is_null() {
                return INVALID_ARGUMENT;
            }
            unsafe { std::ptr::copy_nonoverlapping(rendered.as_ptr(), out, rendered.len()) };
        }
        0
    })
}

/// `node:path`'s decompositions, answered by the one implementation both engines link.
///
/// `part` selects it: 0 `dirname`, 1 `basename`, 2 `extname`, 3 `normalize`. The answer is a
/// slice of the caller's own path for everything but `normalize`, so the shared crate
/// allocates nothing and this copies once, straight into the caller's buffer.
///
/// The caller sizes that buffer from its input -- no decomposition is longer than the path it
/// came from -- so the two-call "ask the length, then ask again" dance is not needed. The
/// length is still reported through `out_len` whether or not it fitted, so a caller that gets
/// the bound wrong can recover instead of truncating.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_path_part(
    part: u32,
    path: *const u8,
    path_len: u64,
    suffix: *const u8,
    suffix_len: u64,
    out: *mut u8,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        let Some(part) = (match part {
            0 => Some(kiln_node_path::PathPart::Dir),
            1 => Some(kiln_node_path::PathPart::Base),
            2 => Some(kiln_node_path::PathPart::Ext),
            3 => Some(kiln_node_path::PathPart::Normalize),
            _ => None,
        }) else {
            return INVALID_ARGUMENT;
        };
        let Some(path) = (unsafe { borrowed_str(path, path_len) }) else {
            return INVALID_ARGUMENT;
        };
        let suffix = if suffix.is_null() {
            None
        } else {
            match unsafe { borrowed_str(suffix, suffix_len) } {
                Some(suffix) => Some(suffix),
                None => return INVALID_ARGUMENT,
            }
        };
        let answer = kiln_node_path::posix_path_part(part, path, suffix);
        unsafe { spill(answer.as_bytes(), out, capacity, out_len) }
    })
}

/// `new URL(input[, base])`, answered by the `url` crate the interpreter links (ADR 0025,
/// ADR 0063 rule 4): the eleven WHATWG components -- href, origin, protocol, username,
/// password, host, hostname, port, pathname, search, hash -- NUL-separated in that order,
/// spilled into the caller's buffer. `URL_INVALID` says the input does not parse, which is
/// the `TypeError` the constructor throws; the other statuses are the ABI's. A component's
/// spelling follows node: `search` and `hash` are empty rather than `?` or `#` when there is
/// nothing after the mark, `port` is empty for the scheme's default, `origin` is `null` for
/// an opaque origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_url_parse(
    input: *const u8,
    input_len: u64,
    base: *const u8,
    base_len: u64,
    out: *mut u8,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        let Some(input) = (unsafe { borrowed_str(input, input_len) }) else {
            return INVALID_ARGUMENT;
        };
        let base = if base.is_null() || base_len == 0 {
            None
        } else {
            match unsafe { borrowed_str(base, base_len) } {
                Some(base) => Some(base),
                None => return INVALID_ARGUMENT,
            }
        };
        let Ok(record) = kiln_url::record(input, base) else {
            return URL_INVALID;
        };
        unsafe { spill(record.as_bytes(), out, capacity, out_len) }
    })
}

/// A URL setter: `field` names the component (0 href, 1 protocol, 2 username, 3 password,
/// 4 host, 5 hostname, 6 port, 7 pathname, 8 search, 9 hash), the answer is the new href
/// record in the same shape `kiln_url_parse` spills. A setter the specification ignores --
/// a scheme change from special to non-special, a port on a URL with no host -- leaves the
/// URL as it was, which is what node does; `href` itself must parse or `URL_INVALID` says so.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_url_set(
    href: *const u8,
    href_len: u64,
    field: u32,
    value: *const u8,
    value_len: u64,
    out: *mut u8,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        let Some(href) = (unsafe { borrowed_str(href, href_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(value) = (unsafe { borrowed_str(value, value_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Ok(record) = kiln_url::set(href, field, value) else {
            return URL_INVALID;
        };
        unsafe { spill(record.as_bytes(), out, capacity, out_len) }
    })
}

/// `String.prototype.toUpperCase` / `toLowerCase` beyond ASCII: the full Unicode mapping,
/// which is Rust's own (`char::to_uppercase`, including the one-to-many cases -- `ß` to
/// `SS`), over UTF-16 in and out. A lone surrogate passes through unchanged. Spilled the way
/// the other string exports are: `out_len` is the answer's length whether or not it fitted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_string_case(
    units: *const u16,
    count: u64,
    upper: u32,
    out: *mut u16,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        if out_len.is_null() || (units.is_null() && count != 0) {
            return INVALID_ARGUMENT;
        }
        let Ok(count) = usize::try_from(count) else {
            return INVALID_ARGUMENT;
        };
        let input = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(units, count) }
        };
        let mut answer: Vec<u16> = Vec::with_capacity(count);
        for decoded in char::decode_utf16(input.iter().copied()) {
            match decoded {
                Ok(c) => {
                    if upper != 0 {
                        for mapped in c.to_uppercase() {
                            let mut buf = [0u16; 2];
                            answer.extend_from_slice(mapped.encode_utf16(&mut buf));
                        }
                    } else {
                        for mapped in c.to_lowercase() {
                            let mut buf = [0u16; 2];
                            answer.extend_from_slice(mapped.encode_utf16(&mut buf));
                        }
                    }
                }
                Err(lone) => answer.push(lone.unpaired_surrogate()),
            }
        }
        unsafe { out_len.write(answer.len() as u64) };
        if capacity < answer.len() as u64 {
            return INVALID_ARGUMENT;
        }
        if !answer.is_empty() {
            if out.is_null() {
                return INVALID_ARGUMENT;
            }
            unsafe { std::ptr::copy_nonoverlapping(answer.as_ptr(), out, answer.len()) };
        }
        0
    })
}

/// The input did not parse as a URL: the constructor's `TypeError`, kept apart from the
/// ABI's own statuses so the caller can throw rather than refuse.
pub const URL_INVALID: i32 = 2;

/// `path.relative(from, to)`, from the same shared implementation.
///
/// Unlike the decompositions this one can be longer than either input -- every component of
/// `from` that `to` does not share becomes `"../"` -- so the caller's bound has to account for
/// that, and `out_len` reports the true length either way.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_path_relative(
    from: *const u8,
    from_len: u64,
    to: *const u8,
    to_len: u64,
    out: *mut u8,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        let (Some(from), Some(to)) = (unsafe { borrowed_str(from, from_len) }, unsafe {
            borrowed_str(to, to_len)
        }) else {
            return INVALID_ARGUMENT;
        };
        let answer = kiln_node_path::posix_path_relative(from, to);
        unsafe { spill(answer.as_bytes(), out, capacity, out_len) }
    })
}

/// A borrowed `&str` over a caller's bytes, or `None` if they are not a string.
unsafe fn borrowed_str<'a>(bytes: *const u8, len: u64) -> Option<&'a str> {
    if bytes.is_null() {
        return (len == 0).then_some("");
    }
    let len = usize::try_from(len).ok()?;
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(bytes, len) }).ok()
}

/// Report `answer`'s length through `out_len` and copy it out when it fits.
unsafe fn spill(answer: &[u8], out: *mut u8, capacity: u64, out_len: *mut u64) -> i32 {
    if out_len.is_null() {
        return INVALID_ARGUMENT;
    }
    unsafe { out_len.write(answer.len() as u64) };
    if capacity < answer.len() as u64 {
        return INVALID_ARGUMENT;
    }
    if !answer.is_empty() {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        unsafe { std::ptr::copy_nonoverlapping(answer.as_ptr(), out, answer.len()) };
    }
    0
}

unsafe fn borrowed_path<'a>(bytes: *const u8, len: u64) -> Option<&'a std::path::Path> {
    if bytes.is_null() {
        return (len == 0).then(|| std::path::Path::new(""));
    }
    let len = usize::try_from(len).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
        Some(std::path::Path::new(std::ffi::OsStr::from_bytes(slice)))
    }
    #[cfg(not(unix))]
    {
        let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
        let s = std::str::from_utf8(slice).ok()?;
        Some(std::path::Path::new(s))
    }
}

fn map_io_error(err: &std::io::Error) -> i32 {
    match err.kind() {
        std::io::ErrorKind::NotFound => 1,
        std::io::ErrorKind::AlreadyExists => 2,
        std::io::ErrorKind::PermissionDenied => 3,
        _ => 6,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_read_file_sync(
    path: *const u8,
    path_len: u64,
    out_bytes: *mut *mut u8,
    out_len: *mut u64,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_bytes.is_null() || out_len.is_null() || out_error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        match std::fs::read(p) {
            Ok(data) => {
                let len = data.len() as u64;
                let mut boxed = data.into_boxed_slice();
                let ptr = boxed.as_mut_ptr();
                core::mem::forget(boxed);
                unsafe {
                    out_bytes.write(ptr);
                    out_len.write(len);
                    out_error_code.write(0);
                }
                0
            }
            Err(err) => {
                unsafe {
                    out_bytes.write(core::ptr::null_mut());
                    out_len.write(0);
                    out_error_code.write(map_io_error(&err));
                }
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_free_bytes(bytes: *mut u8, len: u64) {
    if !bytes.is_null() && len > 0 {
        if let Ok(len) = usize::try_from(len) {
            let _ = unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(bytes, len)) };
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_write_file_sync(
    path: *const u8,
    path_len: u64,
    data: *const u8,
    data_len: u64,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        let slice = if data.is_null() {
            &[]
        } else {
            let Ok(len) = usize::try_from(data_len) else {
                return INVALID_ARGUMENT;
            };
            unsafe { std::slice::from_raw_parts(data, len) }
        };
        match std::fs::write(p, slice) {
            Ok(()) => {
                unsafe { out_error_code.write(0) };
                0
            }
            Err(err) => {
                unsafe { out_error_code.write(map_io_error(&err)) };
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_mkdir_sync(
    path: *const u8,
    path_len: u64,
    recursive: i32,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        let res = if recursive != 0 {
            std::fs::create_dir_all(p)
        } else {
            std::fs::create_dir(p)
        };
        match res {
            Ok(()) => {
                unsafe { out_error_code.write(0) };
                0
            }
            Err(err) => {
                unsafe { out_error_code.write(map_io_error(&err)) };
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_stat_sync(
    path: *const u8,
    path_len: u64,
    follow_symlinks: i32,
    out_size: *mut u64,
    out_flags: *mut u32,
    out_mtime_ms: *mut f64,
    out_mode: *mut u32,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_size.is_null()
            || out_flags.is_null()
            || out_mtime_ms.is_null()
            || out_mode.is_null()
            || out_error_code.is_null()
        {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        let res = if follow_symlinks != 0 {
            std::fs::metadata(p)
        } else {
            std::fs::symlink_metadata(p)
        };
        match res {
            Ok(m) => {
                let mut flags: u32 = 0;
                if m.is_file() {
                    flags |= 1 << 3;
                }
                if m.is_dir() {
                    flags |= 1 << 4;
                }
                if m.is_symlink() {
                    flags |= 1 << 5;
                }
                let mtime_ms = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0.0, |d| {
                        d.as_secs_f64() * 1000.0 + (d.subsec_nanos() as f64) / 1_000_000.0
                    });
                #[cfg(unix)]
                let mode = std::os::unix::fs::MetadataExt::mode(&m);
                #[cfg(not(unix))]
                let mode = if m.is_dir() { 0o40755 } else { 0o100644 };

                unsafe {
                    out_size.write(m.len());
                    out_flags.write(flags);
                    out_mtime_ms.write(mtime_ms);
                    out_mode.write(mode);
                    out_error_code.write(0);
                }
                0
            }
            Err(err) => {
                unsafe { out_error_code.write(map_io_error(&err)) };
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_readdir_sync(
    path: *const u8,
    path_len: u64,
    out_entries: *mut *mut u8,
    out_len: *mut u64,
    out_count: *mut u64,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_entries.is_null()
            || out_len.is_null()
            || out_count.is_null()
            || out_error_code.is_null()
        {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        let dir = match std::fs::read_dir(p) {
            Ok(d) => d,
            Err(err) => {
                unsafe {
                    out_entries.write(core::ptr::null_mut());
                    out_len.write(0);
                    out_count.write(0);
                    out_error_code.write(map_io_error(&err));
                }
                return 0;
            }
        };
        let mut buf: Vec<u8> = Vec::new();
        let mut count: u64 = 0;
        for entry in dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name_bytes = name.as_encoded_bytes();
            let ft = entry.file_type().ok();
            let mut flags: u8 = 0;
            if let Some(ft) = ft {
                if ft.is_file() {
                    flags |= 1;
                }
                if ft.is_dir() {
                    flags |= 2;
                }
                if ft.is_symlink() {
                    flags |= 4;
                }
            }
            let name_len = name_bytes.len().min(u16::MAX as usize);
            let Ok(name_len_u16) = u16::try_from(name_len) else {
                continue;
            };
            buf.extend_from_slice(&name_len_u16.to_le_bytes());
            buf.push(flags);
            if let Some(sub) = name_bytes.get(..name_len) {
                buf.extend_from_slice(sub);
            }
            count = count.saturating_add(1);
        }
        let len = buf.len() as u64;
        let mut boxed = buf.into_boxed_slice();
        let ptr = boxed.as_mut_ptr();
        core::mem::forget(boxed);
        unsafe {
            out_entries.write(ptr);
            out_len.write(len);
            out_count.write(count);
            out_error_code.write(0);
        }
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_realpath_sync(
    path: *const u8,
    path_len: u64,
    out_bytes: *mut *mut u8,
    out_len: *mut u64,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_bytes.is_null() || out_len.is_null() || out_error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        match std::fs::canonicalize(p) {
            Ok(canon) => {
                let bytes = canon.to_string_lossy().into_owned().into_bytes();
                let len = bytes.len() as u64;
                let mut boxed = bytes.into_boxed_slice();
                let ptr = boxed.as_mut_ptr();
                core::mem::forget(boxed);
                unsafe {
                    out_bytes.write(ptr);
                    out_len.write(len);
                    out_error_code.write(0);
                }
                0
            }
            Err(err) => {
                unsafe {
                    out_bytes.write(core::ptr::null_mut());
                    out_len.write(0);
                    out_error_code.write(map_io_error(&err));
                }
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_unlink_sync(
    path: *const u8,
    path_len: u64,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        match std::fs::remove_file(p) {
            Ok(()) => {
                unsafe { out_error_code.write(0) };
                0
            }
            Err(err) => {
                unsafe { out_error_code.write(map_io_error(&err)) };
                0
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_node_fs_rmdir_sync(
    path: *const u8,
    path_len: u64,
    out_error_code: *mut i32,
) -> i32 {
    ffi(|| {
        if out_error_code.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(p) = (unsafe { borrowed_path(path, path_len) }) else {
            unsafe { out_error_code.write(1) };
            return 0;
        };
        match std::fs::remove_dir(p) {
            Ok(()) => {
                unsafe { out_error_code.write(0) };
                0
            }
            Err(err) => {
                unsafe { out_error_code.write(map_io_error(&err)) };
                0
            }
        }
    })
}

/// Interns a registered-symbol key in the active Realm. The input is the canonical
/// code-unit header, not a UTF-8 byte spelling; the returned identity is stable for that
/// Realm and deliberately meaningless outside it. Generated modules never own registry
/// state independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_symbol_for(
    key: *const super::KilnString,
    out_id: *mut u64,
) -> i32 {
    ffi(|| {
        if out_id.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(key) = (unsafe { super::validate_string(key) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let borrowed_key = BorrowedStringView(key);
        if let Some(id) = realm.symbols.borrow().get(&borrowed_key).copied() {
            unsafe { out_id.write(id) };
            return 0;
        }
        let Ok(id) = NEXT_SYMBOL.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        }) else {
            return RUNTIME_ERROR;
        };
        let owned = StringUnits::from_view(key);
        realm.symbols.borrow_mut().insert(owned.clone(), id);
        realm.symbol_keys.borrow_mut().insert(id, owned);
        unsafe { out_id.write(id) };
        0
    })
}

/// Writes a canonical header borrowing a registered-symbol key from the active Realm. The
/// header and its data remain valid until the active Realm is torn down; C callers copy code
/// units before leaving that lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_symbol_key_for(
    id: u64,
    out_key: *mut super::KilnString,
) -> i32 {
    ffi(|| {
        if out_key.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let keys = realm.symbol_keys.borrow();
        let Some(key) = keys.get(&id) else {
            return INVALID_ARGUMENT;
        };
        let (flags, data) = match key.as_view() {
            kiln_string::StringView::Latin1(bytes) => (
                0,
                if bytes.is_empty() {
                    core::ptr::null()
                } else {
                    bytes.as_ptr().cast()
                },
            ),
            kiln_string::StringView::Utf16(units) => (
                1,
                if units.is_empty() {
                    core::ptr::null()
                } else {
                    units.as_ptr().cast()
                },
            ),
        };
        unsafe {
            out_key.write(super::KilnString {
                unit_len: key.unit_len() as u32,
                flags,
                data,
            });
        }
        0
    })
}

/// Returns the realm-wide opaque property store for one runtime-provided callable.
///
/// A null candidate performs an allocation-free lookup. Otherwise the first candidate for
/// `(tag, code)` is published and its destructor transfers to the realm. Later callers
/// receive that exact pointer and retain ownership of their unpublished candidate. Rust
/// never dereferences this private C object, so layout stays behind the ABI while identity
/// and lifetime become realm-wide.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_callable_store(
    tag: u32,
    code: u64,
    candidate: *mut c_void,
    drop_candidate: Option<DropWork>,
    out_store: *mut *mut c_void,
) -> i32 {
    ffi(|| {
        if out_store.is_null() || (candidate.is_null() != drop_candidate.is_none()) {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let mut stores = realm.callable_stores.borrow_mut();
        if let Some(store) = stores.get(&(tag, code)) {
            unsafe { out_store.write(store.pointer) };
            return 0;
        }
        let Some(drop) = drop_candidate else {
            unsafe { out_store.write(ptr::null_mut()) };
            return 0;
        };
        stores.insert(
            (tag, code),
            CallableStore {
                pointer: candidate,
                drop,
            },
        );
        unsafe { out_store.write(candidate) };
        0
    })
}

/// Returns one zero-initialized, realm-owned slot for a private generated-C value.
///
/// Rust deliberately treats the slot as four opaque machine words: it owns identity and
/// lifetime, while the generated C kernel remains the sole authority for `kiln_value`'s
/// private layout. Stable numeric keys are assigned by that kernel to realm singletons such
/// as `globalThis`, `process`, and `Atomics`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_value_slot(key: u32, out_slot: *mut *mut c_void) -> i32 {
    ffi(|| {
        if out_slot.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let mut slots = realm.value_slots.borrow_mut();
        let slot = slots.entry(key).or_insert_with(|| Box::new([0; 4]));
        unsafe { out_slot.write(slot.as_mut_ptr().cast()) };
        0
    })
}

/// Records the command line one launcher's `main` was handed, on a realm that is not yet
/// entered.
///
/// The strings are copied here, which is what makes this safe to call with the OS's own
/// `argv`: nothing in the realm afterwards depends on that array, or on the environment
/// block behind it, still being alive. `argc` is taken as it arrived, so `argv[0]` is the
/// executable path exactly as the OS spelled it rather than a path this crate resolved --
/// resolving it would make two invocations of the same binary disagree about their own name.
///
/// # Safety
///
/// `argv` must point to `argc` NUL-terminated C strings, all live for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_set_argv(
    handle: u64,
    argc: u64,
    argv: *const *const c_char,
) -> i32 {
    ffi(|| {
        let Some(realm) = realm(handle) else {
            return INVALID_ARGUMENT;
        };
        // A realm that is already running would be handing out pointers into the vector this
        // call replaces. The launcher sets the command line once, before it enters.
        if realm.entered.get() {
            return INVALID_ARGUMENT;
        }
        let Ok(count) = usize::try_from(argc) else {
            return INVALID_ARGUMENT;
        };
        if count != 0 && argv.is_null() {
            return INVALID_ARGUMENT;
        }
        let mut copied: Vec<Box<[u8]>> = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: the caller promises `argc` live C strings at `argv`.
            let entry = unsafe { *argv.add(index) };
            if entry.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: as above; the string is NUL-terminated and live for this call.
            let bytes = unsafe { CStr::from_ptr(entry) }.to_bytes();
            copied.push(bytes.to_vec().into_boxed_slice());
        }
        *realm.argv.borrow_mut() = copied;
        0
    })
}

/// Reports how many command-line arguments the active Realm was given.
///
/// Zero is the honest answer for an embedded realm: nothing called `main`, so there is no
/// command line to report and `process.argv` stays empty rather than inventing one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_argv_count(out_count: *mut u64) -> i32 {
    ffi(|| {
        if out_count.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let count = realm.argv.borrow().len() as u64;
        unsafe { out_count.write(count) };
        0
    })
}

/// Borrows one command-line argument from the active Realm.
///
/// The bytes stay valid for the life of the realm, which is longer than the generated
/// prelude needs them: it copies each one into a language string as it builds `process.argv`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_argv_at(
    index: u64,
    out_bytes: *mut *const u8,
    out_len: *mut u64,
) -> i32 {
    ffi(|| {
        if out_bytes.is_null() || out_len.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Ok(index) = usize::try_from(index) else {
            return INVALID_ARGUMENT;
        };
        let argv = realm.argv.borrow();
        let Some(entry) = argv.get(index) else {
            return INVALID_ARGUMENT;
        };
        unsafe {
            out_bytes.write(entry.as_ptr());
            out_len.write(entry.len() as u64);
        }
        0
    })
}

/// Advances deterministic SplitMix64 state owned by the active realm.
///
/// The seed is an explicit generated-artifact capability. It must agree on every draw in a
/// realm, preventing separately compiled modules from silently starting independent streams.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_entropy_word(seed: u64, out_word: *mut u64) -> i32 {
    ffi(|| {
        if out_word.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let state = match realm.entropy.get() {
            Some((configured, _)) if configured != seed => return INVALID_ARGUMENT,
            Some((_, state)) => state,
            None => seed,
        };
        let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        realm.entropy.set(Some((seed, state)));
        let mut mixed = state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^= mixed >> 31;
        unsafe { out_word.write(mixed) };
        0
    })
}

/// How many fields [`kiln_realm_runtime_report`] writes.
const RUNTIME_REPORT_FIELDS: usize = 9;

/// Reports whole-runtime and HTTP-connection counters for the `SIGUSR1` dump.
///
/// The prelude assembles the dump because it owns the heap layout, and these numbers are on
/// the other side of the archive boundary from it, so they cross as a slot array rather than
/// as a struct nothing would keep identical in both languages. `capacity` in and a count out
/// means a prelude built against a shorter report still reads the prefix it knows.
///
/// | # | Field |
/// | --- | --- |
/// | 0 | live tasks |
/// | 1 | park count |
/// | 2 | busy duration, microseconds |
/// | 3 | elapsed, microseconds |
/// | 4 | global queue depth |
/// | 5 | worker threads |
/// | 6 | open server connections |
/// | 7 | open client connections |
/// | 8 | requests in flight |
///
/// Fields 0 to 5 come from one `sample_runtime_metrics` call and are *interval* counters:
/// they cover the time since the previous report, or since the runtime was built for the
/// first one. Fields 6 to 8 are instantaneous.
///
/// # Safety
///
/// `out_fields` must be writable for `capacity` `uint64_t`, and `out_count` for one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_runtime_report(
    out_fields: *mut u64,
    capacity: u64,
    out_count: *mut u64,
) -> i32 {
    ffi(|| {
        if out_fields.is_null() || out_count.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null; the caller owns the output slot.
        unsafe { out_count.write(0) };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        let metrics = realm.runtime.executor().sample_runtime_metrics();
        let fields: [u64; RUNTIME_REPORT_FIELDS] = [
            metrics.live_tasks as u64,
            metrics.park_count,
            metrics.busy_duration.as_micros().min(u128::from(u64::MAX)) as u64,
            metrics.elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
            metrics.global_queue_depth as u64,
            metrics.workers as u64,
            runtime.open_server_connections() as u64,
            runtime.open_client_connections() as u64,
            runtime.requests_in_flight() as u64,
        ];
        let written = usize::try_from(capacity)
            .unwrap_or(RUNTIME_REPORT_FIELDS)
            .min(RUNTIME_REPORT_FIELDS);
        // SAFETY: the caller promised `capacity` writable slots and this writes no more.
        unsafe {
            std::ptr::copy_nonoverlapping(fields.as_ptr(), out_fields, written);
            out_count.write(written as u64);
        }
        0
    })
}

/// Attaches the prelude's collector to the active realm (ADR 0041).
///
/// With a null `candidate` this only reports the realm's heap: the existing one, or null when
/// none has been created yet. With a candidate it adopts it when the realm has no heap and
/// otherwise reports the existing heap, in which case the caller frees its candidate.
///
/// # Safety
///
/// `hooks` must point at a hooks table that stays valid for the life of the program, and
/// `candidate` must be a heap whose first word is that hooks pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_attach(
    hooks: *const GcHooks,
    candidate: *mut c_void,
    out_heap: *mut *mut c_void,
) -> i32 {
    ffi(|| {
        if out_heap.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null; the caller owns the output slot.
        unsafe { out_heap.write(std::ptr::null_mut()) };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let mut gc = realm.gc.borrow_mut();
        if gc.heap.is_null() {
            if candidate.is_null() {
                return 0;
            }
            if hooks.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: checked non-null; the prelude's table is static.
            let table = unsafe { *hooks };
            if table.version != GC_HOOKS_VERSION
                || (table.size as usize) < std::mem::size_of::<GcHooks>()
                || table.collect.is_none()
                || table.teardown_begin.is_none()
                || table.teardown_end.is_none()
                || table.mark_value.is_none()
                || table.mark_block.is_none()
                || table.introspect.is_none()
            {
                return INVALID_ARGUMENT;
            }
            gc.heap = candidate;
            gc.hooks = Some(table);
            ACTIVE_HEAP.with(|slot| slot.set(candidate));
            bump_generation();
        }
        // SAFETY: checked non-null above.
        unsafe { out_heap.write(gc.heap) };
        0
    })
}

/// The active realm's collector heap, or null: one thread-local load, no lookup and no
/// unwind guard, because the allocator asks on every allocation.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_gc_current_heap() -> *mut c_void {
    ACTIVE_HEAP.with(|slot| slot.get())
}

/// Bytes reserved for the prelude's process-wide shared state (ADR 0071).
const PROCESS_SHARED_BYTES: usize = 4096;

/// The prelude's process-wide block: shared keys and the shape root. Plain zeroed storage
/// whose layout the prelude owns and stamps; the archive only guarantees there is one, at one
/// address, for the whole process -- which a prelude static cannot, because the prelude is
/// emitted per translation unit with internal linkage.
#[repr(C, align(64))]
struct ProcessShared(std::cell::UnsafeCell<[u8; PROCESS_SHARED_BYTES]>);

// SAFETY: the archive never reads or writes the bytes; the prelude synchronizes every access
// to them itself (a spin lock for writers, acquire/release publication for readers).
unsafe impl Sync for ProcessShared {}

static PROCESS_SHARED: ProcessShared =
    ProcessShared(std::cell::UnsafeCell::new([0; PROCESS_SHARED_BYTES]));

/// The process-wide block, or null when the prelude asks for more than is reserved.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_process_shared(size: u64) -> *mut c_void {
    if size > PROCESS_SHARED_BYTES as u64 {
        return std::ptr::null_mut();
    }
    PROCESS_SHARED.0.get().cast()
}

/// Registers a root function the collector calls on every mark, and at teardown when
/// `flags` says the roots it marks must outlive the realm. Registering the same function and
/// context twice is idempotent.
///
/// # Safety
///
/// `roots` and `context` must stay valid until the realm is destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_register_roots(
    roots: Option<GcRootsFn>,
    context: *mut c_void,
    flags: u32,
) -> i32 {
    register_gc_roots(roots, None, context, flags)
}

/// Registers the same mark/teardown callback as `kiln_realm_gc_register_roots`, plus a
/// read-only enumerator of exactly its live roots. The enumerator must neither mark nor change
/// the realm. Registrations without an enumerator still force ownership proof to refuse.
///
/// # Safety
///
/// Both callbacks and `context` must stay valid until the realm is destroyed. `visit_roots`
/// must call `visit` for every root its mark callback could mark at a turn safepoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_register_roots_with_visitor(
    roots: Option<GcRootsFn>,
    visit_roots: Option<GcRootsVisitFn>,
    context: *mut c_void,
    flags: u32,
) -> i32 {
    if visit_roots.is_none() {
        return INVALID_ARGUMENT;
    }
    register_gc_roots(roots, visit_roots, context, flags)
}

fn register_gc_roots(
    roots: Option<GcRootsFn>,
    visit_roots: Option<GcRootsVisitFn>,
    context: *mut c_void,
    flags: u32,
) -> i32 {
    ffi(|| {
        let Some(roots) = roots else {
            return INVALID_ARGUMENT;
        };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let mut gc = realm.gc.borrow_mut();
        if let Some(existing) = gc
            .roots
            .iter()
            .find(|root| root.roots as usize == roots as usize && root.context == context)
        {
            return if existing.visit.map(|visit| visit as usize)
                == visit_roots.map(|visit| visit as usize)
            {
                0
            } else {
                INVALID_ARGUMENT
            };
        }
        if gc.roots.try_reserve(1).is_err() {
            return RUNTIME_ERROR;
        }
        gc.roots.push(GcRoot {
            roots,
            visit: visit_roots,
            context,
            flags,
        });
        0
    })
}

/// Removes a root registration whose memory is about to go away, so a later collection
/// does not call it. Unknown pairs are ignored.
///
/// # Safety
///
/// Only the registration's own `roots`/`context` pair may be passed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_unregister_roots(
    roots: Option<GcRootsFn>,
    context: *mut c_void,
) -> i32 {
    ffi(|| {
        let Some(roots) = roots else {
            return INVALID_ARGUMENT;
        };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        realm
            .gc
            .borrow_mut()
            .roots
            .retain(|root| !(root.roots as usize == roots as usize && root.context == context));
        0
    })
}

/// Marks every root the archive and the registered root functions hold, through the hooks
/// of `heap`. Called by the prelude's collector at the start of a collection.
///
/// # Safety
///
/// `heap` must be the heap attached to the active realm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_mark_roots(heap: *mut c_void) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        mark_realm_roots(&realm, heap)
    })
}

/// Visits the provider's own GC roots without marking or allocating in the sender heap.
/// The callback receives a live `kiln_value` slot (`GC_VISIT_VALUE`) or a block payload pointer
/// (`GC_VISIT_BLOCK`). Paired registered callbacks then enumerate their roots through the same
/// visitor. Return zero only when all provider and registered roots were enumerated, one when a
/// legacy mark-only registration remains, or an error on invalid input/visitor failure. A caller
/// must refuse ownership transfer on every nonzero result. Mark callbacks are never invoked here.
///
/// # Safety
///
/// `heap` must be attached to the active realm; `visit` and `context` must remain valid through
/// this call. The visitor may inspect but must not mutate roots or reenter the realm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_visit_provider_roots(
    heap: *mut c_void,
    visit: Option<GcVisitRoot>,
    context: *mut c_void,
) -> i32 {
    ffi(|| {
        let Some(visit) = visit else {
            return INVALID_ARGUMENT;
        };
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let registered_roots = {
            let gc = realm.gc.borrow();
            if heap.is_null() || gc.heap != heap || gc.hooks.is_none() {
                return INVALID_ARGUMENT;
            }
            gc.roots
                .iter()
                .map(|root| (root.visit, root.context))
                .collect::<Vec<_>>()
        };
        let visitor = GcVisitor {
            visit,
            context,
            failed: std::cell::Cell::new(false),
        };
        for_each_provider_root(&realm, &visitor);
        let mut incomplete = false;
        for (roots, root_context) in registered_roots {
            if let Some(roots) = roots {
                // SAFETY: registration promises a live, non-mutating root enumerator.
                if unsafe {
                    roots(
                        root_context,
                        relay_registered_root,
                        (&raw const visitor).cast_mut().cast(),
                    )
                } != 0
                {
                    return RUNTIME_ERROR;
                }
            } else {
                incomplete = true;
            }
        }
        if visitor.failed.get() {
            return RUNTIME_ERROR;
        }
        if incomplete {
            GC_VISIT_REGISTERED_ROOTS_UNENUMERATED
        } else {
            0
        }
    })
}

/// Marks one value slot through `heap`'s hooks, for root functions outside the prelude.
///
/// # Safety
///
/// `heap` must be a live attached heap and `value` a readable `kiln_value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_mark_value(heap: *mut c_void, value: *const c_void) -> i32 {
    ffi(|| {
        let Some(hooks) = gc_hooks_of(heap) else {
            return INVALID_ARGUMENT;
        };
        GcMarker { heap, hooks }.value(value);
        0
    })
}

/// Marks one heap block through `heap`'s hooks, for root functions outside the prelude.
///
/// # Safety
///
/// `heap` must be a live attached heap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_gc_mark_block(heap: *mut c_void, pointer: *mut c_void) -> i32 {
    ffi(|| {
        let Some(hooks) = gc_hooks_of(heap) else {
            return INVALID_ARGUMENT;
        };
        GcMarker { heap, hooks }.block(pointer);
        0
    })
}

/// Switches a realm to the host's wall clock (ADR 0044) before it is entered.
///
/// Every realm starts on the deterministic virtual clock; a launcher built with the
/// host-clock policy calls this once, right after `kiln_realm_create`. From then on
/// `Date.now()` is Unix time, and timers fire when their wall-clock deadline passes whether
/// or not the realm is idle. `INVALID_ARGUMENT` for an unknown handle or an entered realm.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_use_wall_clock(handle: u64) -> i32 {
    ffi(|| {
        let Some(realm) = realm(handle) else {
            return INVALID_ARGUMENT;
        };
        if realm.entered.get() {
            return INVALID_ARGUMENT;
        }
        realm.runtime.state.borrow_mut().use_host_clock();
        realm.clock.set(NativeClock::wall());
        // The realm's clock jumps from the epoch to the present here, before any timer
        // measures from it.
        tick_clock(&realm);
        0
    })
}

/// Writes the active Realm's clock in ECMAScript milliseconds: the virtual instant, or the
/// host's wall clock for a realm switched by [`kiln_realm_use_wall_clock`].
///
/// # Safety
///
/// `out_milliseconds` must be a writable `f64` slot owned by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_now(out_milliseconds: *mut f64) -> i32 {
    ffi(|| {
        if out_milliseconds.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(now) = realm_clock_reading(&realm) else {
            return RUNTIME_ERROR;
        };
        let milliseconds = kiln_runtime::epoch_milliseconds(now);
        // SAFETY: null was rejected and the caller supplies a writable slot.
        unsafe { out_milliseconds.write(milliseconds) };
        0
    })
}

/// Writes the active Realm's monotonic elapsed clock in milliseconds.
///
/// A virtual realm begins at zero and advances with its scheduled work. A wall-clock realm
/// measures from the instant it was enabled, never from the Unix epoch; this is the clock
/// exposed as `performance.now()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_performance_now(out_milliseconds: *mut f64) -> i32 {
    ffi(|| {
        if out_milliseconds.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let elapsed = match realm.clock.get() {
            NativeClock::Virtual => realm.work.try_borrow().ok().map(|work| work.now()),
            NativeClock::Wall { started, .. } => Some(started.elapsed()),
        };
        let Some(elapsed) = elapsed else {
            return RUNTIME_ERROR;
        };
        // SAFETY: null was rejected and the caller supplies a writable slot.
        unsafe { out_milliseconds.write(elapsed.as_secs_f64() * 1_000.0) };
        0
    })
}

/// Writes the active Realm's deterministic clock in whole epoch nanoseconds.
///
/// Split in two because C99 has no portable 128-bit integer: `out_high` takes the signed top
/// half and `out_low` the unsigned bottom half of the count.
///
/// `Temporal` measures instants in nanoseconds, and [`kiln_realm_now`] answers milliseconds --
/// so feeding one to the other would round the realm's clock and make the two engines disagree
/// below the millisecond. The realm has the resolution; this is the accessor that carries it.
/// The clock is still the realm's virtual one, which ADR 0015 starts at the epoch.
///
/// # Safety
///
/// `out_high` and `out_low` must be writable slots owned by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_now_nanoseconds(out_high: *mut i64, out_low: *mut u64) -> i32 {
    ffi(|| {
        if out_high.is_null() || out_low.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(now) = realm_clock_reading(&realm) else {
            return RUNTIME_ERROR;
        };
        let nanoseconds = now.as_nanos() as i128;
        // SAFETY: null was rejected and the caller supplies writable slots.
        unsafe { out_high.write((nanoseconds >> 64) as i64) };
        // SAFETY: as above.
        unsafe { out_low.write(nanoseconds as u64) };
        0
    })
}

/// Queues one opaque FIFO microtask on the active Realm.
///
/// # Safety
///
/// `work` must be owned storage accepted by `run` and `drop`. Ownership transfers on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_queue_microtask(
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Ok(mut queue) = realm.work.try_borrow_mut() else {
            return RUNTIME_ERROR;
        };
        let Some(work) = NativeMicrotask::new(work, run, drop) else {
            return INVALID_ARGUMENT;
        };
        queue.queue_microtask(work);
        0
    })
}

/// Queues one opaque FIFO Node next-tick callback on the active Realm.
///
/// Next-tick work is a separate language lane, drained before Promise jobs while retaining
/// the same Realm owner, callback ABI, and disposal rules as ordinary microtasks.
///
/// # Safety
///
/// `work` must be owned storage accepted by `run` and `drop`. Ownership transfers on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_queue_next_tick(
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Ok(mut queue) = realm.work.try_borrow_mut() else {
            return RUNTIME_ERROR;
        };
        let Some(work) = NativeMicrotask::new(work, run, drop) else {
            return INVALID_ARGUMENT;
        };
        queue.queue_next_tick(work);
        0
    })
}

/// Registers the prelude's promise job lane with the active Realm (Phase 6.3).
///
/// The prelude calls this when its collector heap attaches, which is once per realm, and the
/// context is that heap. The realm keeps the pair and calls it from the drain: promise jobs
/// run after `process.nextTick` and before the next host completion, which is where they ran
/// when each one arrived here as a separate work item.
///
/// # Safety
///
/// `run` and `pending` must stay callable, and `context` must stay readable by them, for as
/// long as the realm lives or until a later registration replaces them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_set_promise_lane(
    run: Option<RunPromiseLane>,
    pending: Option<PromiseLanePending>,
    context: *mut c_void,
) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let (Some(run), Some(pending)) = (run, pending) else {
            return INVALID_ARGUMENT;
        };
        if context.is_null() {
            return INVALID_ARGUMENT;
        }
        realm.promise_lane.set(Some(PromiseLane {
            run,
            pending,
            context,
        }));
        0
    })
}

/// Starts one asynchronous file read on the active Realm.
///
/// The path is copied before this call returns. The continuation and its disposer are owned by
/// the Realm until completion, cancellation, or destruction. Completion payload pointers are
/// borrowed only while `run` executes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_read_file(
    handle: u64,
    path: *const u8,
    path_len: u64,
    run: Option<RunCompletion>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_request: *mut u64,
) -> i32 {
    let operation = HostOpView {
        operation: OP_READ_FILE,
        flags: 1,
        resource: 0,
        argument: 0,
        text: path,
        text_len: path_len,
        bytes: std::ptr::null(),
        bytes_len: 0,
    };
    // SAFETY: this wrapper preserves the original path/callback ownership contract and the
    // operation frame is live for the complete submission call.
    unsafe { kiln_realm_start_host_op(handle, &raw const operation, run, drop, work, out_request) }
}

/// Starts one portable asynchronous host operation on the active Realm.
///
/// Every pointer in `operation` is borrowed only for this call. The provider copies path,
/// host, and payload bytes before returning; continuation ownership transfers only on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_start_host_op(
    handle: u64,
    operation: *const HostOpView,
    run: Option<RunCompletion>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_request: *mut u64,
) -> i32 {
    ffi(|| {
        if operation.is_null() || out_request.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(run) = run else {
            return INVALID_ARGUMENT;
        };
        let Some(drop) = drop else {
            return INVALID_ARGUMENT;
        };
        if work.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_handle(handle) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: null was rejected and the operation frame is borrowed for this call.
        let operation_view = unsafe { &*operation };
        // SAFETY: both framed byte spans remain live until this submission returns.
        let Some(operation) = (unsafe { host_operation(operation_view) }) else {
            return INVALID_ARGUMENT;
        };
        #[cfg(feature = "realm-http")]
        if let AsyncHostOp::CloseResource { resource } = &operation
            && realm.http_connections.borrow().contains_key(resource)
        {
            // Once the realm has attached Hyper, protocol shutdown owns this TCP resource.
            // A raw HostOps close can discard bytes Hyper has selected but not yet submitted,
            // which is exactly the authority split ADR 0032 forbids. Generated adapters must
            // request graceful/abort HTTP close through the dedicated realm operation.
            return INVALID_ARGUMENT;
        }
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        // The callback runs synchronously inside the drive (or `kiln_realm_enter`), so the
        // executor's context is current and the realm's state is not borrowed.
        // `flags & 1` is the generic request-liveness bit; zero leaves incidental inspection
        // requests unreferenced while resource creation still records its own resource state.
        let referenced = operation_view.flags & 1 != 0;
        if http_trace_enabled() {
            eprintln!("[kiln-host] start {operation:?} referenced={referenced}");
        }
        realm.pin(AFFINITY_HOST_OP);
        let request = runtime.start_host_op(operation, referenced);
        std::mem::drop(runtime);
        let continuation = AsyncContinuation {
            pointer: work,
            run,
            drop,
        };
        realm
            .continuations
            .borrow_mut()
            .insert(request, continuation);
        // SAFETY: null was rejected and the caller supplies a writable request slot.
        unsafe { out_request.write(request.get()) };
        0
    })
}

/// Starts one portable operation on the realm currently executing generated code.
///
/// This is the capability-shaped form used by runtime modules: being inside the active realm
/// is the authority to submit work, so generated code never needs to learn or retain its
/// provider-private handle.
///
/// # Safety
///
/// The operation and continuation arguments have the same contract as
/// [`kiln_realm_start_host_op`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_start_active_host_op(
    operation: *const HostOpView,
    run: Option<RunCompletion>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_request: *mut u64,
) -> i32 {
    let handle = ACTIVE_REALM.with(Cell::get);
    // SAFETY: this function preserves the pointed-to frame and continuation contracts.
    unsafe { kiln_realm_start_host_op(handle, operation, run, drop, work, out_request) }
}

/// Cancels one asynchronous request on the active Realm.
///
/// Returns zero when the request was canceled and one when it was already complete or unknown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_cancel_request(handle: u64, request: u64) -> i32 {
    ffi(|| {
        let Some(realm) = active_handle(handle) else {
            return INVALID_ARGUMENT;
        };
        let request = RequestId::new(request);
        let Some(canceled) = realm
            .runtime
            .state_mut()
            .map(|mut state| state.cancel_host_op(request))
        else {
            return RUNTIME_ERROR;
        };
        let removed = realm.continuations.borrow_mut().remove(&request).is_some();
        let queued = realm.queued.borrow().contains(&request);
        if canceled || removed || queued {
            realm.cancelled.borrow_mut().insert(request);
            0
        } else {
            1
        }
    })
}

/// Cancels a request owned by the currently executing realm.
///
/// # Safety
///
/// `request` must be an identity previously returned to this active realm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_cancel_active_request(request: u64) -> i32 {
    let handle = ACTIVE_REALM.with(Cell::get);
    // SAFETY: the active-handle wrapper performs the same identity validation.
    unsafe { kiln_realm_cancel_request(handle, request) }
}

/// Runs every queued next-tick job, promise job and host completion. This is the checkpoint
/// the drive performs between turns and the one the HTTP engine performs after every direct
/// callback (ADR 0042); both are in context.
///
/// Three lanes in one order, and the order is node's. `process.nextTick` outranks everything.
/// The promise lane is the prelude's own queue (Phase 6.3) and it is drained *whole* -- a job
/// that queues a job runs it in the same checkpoint, which is what makes a `then` chain
/// finish before anything else gets a turn -- and only then does a host completion run, so a
/// completion that settles a promise sees that promise's reactions run before the completion
/// after it rather than after both.
fn drain_realm_queues(realm: &NativeRealm) -> i32 {
    let (host, thrown) = realm.drive_context.get();
    loop {
        let next_tick = realm.work.borrow_mut().pop_next_tick();
        if let Some(work) = next_tick {
            let status = work.run(host, thrown);
            if status != 0 {
                return status;
            }
            continue;
        }
        if let Some(lane) = realm.promise_lane.get()
            && lane.pending()
        {
            let status = lane.run(host, thrown);
            if status != 0 {
                return status;
            }
            continue;
        }
        let microtask = realm.work.borrow_mut().pop_microtask();
        if let Some(work) = microtask {
            let status = work.run(host, thrown);
            if status != 0 {
                return status;
            }
            continue;
        }
        return 0;
    }
}

/// Whether a checkpoint would run anything: either lane the realm owns, or the prelude's.
#[cfg(feature = "realm-http")]
fn realm_has_queued_work(realm: &NativeRealm) -> bool {
    if realm.work.borrow().has_queued_work() {
        return true;
    }
    realm.promise_lane.get().is_some_and(PromiseLane::pending)
}

/// The realm's side of a direct connection, server or client: protocol events become the
/// private ABI's event view and go straight to the generated C handler; the checkpoint
/// drains the queues.
#[cfg(feature = "realm-http")]
struct NativeHttpDispatch {
    realm: std::rc::Weak<NativeRealm>,
    resource: kiln_host_abi::ResourceId,
    owner: Rc<HttpEventOwner>,
    /// The connection belongs to a fetch-style (`serve`) server, which takes each request
    /// whole (`HttpDispatch::whole_requests`).
    whole_requests: bool,
}

#[cfg(feature = "realm-http")]
impl kiln_runtime::HttpDispatch for NativeHttpDispatch {
    fn event(&self, event: &kiln_runtime::HttpEvent<'_>) -> i32 {
        use kiln_runtime::HttpEventKind;
        let Some(realm) = self.realm.upgrade() else {
            return INVALID_ARGUMENT;
        };
        // The engine calls the language while the drive is parked (offline, see `reclaim`).
        let _online = reclaim::online();
        tick_clock(&realm);
        let (host, thrown) = realm.drive_context.get();
        let kind = match event.kind {
            HttpEventKind::RequestHead => HTTP_EVENT_REQUEST_HEAD,
            HttpEventKind::RequestData => HTTP_EVENT_REQUEST_DATA,
            HttpEventKind::RequestEnd => HTTP_EVENT_REQUEST_END,
            HttpEventKind::Drain => HTTP_EVENT_DRAIN,
            HttpEventKind::Finish => HTTP_EVENT_FINISH,
            HttpEventKind::ResponseHead => HTTP_EVENT_RESPONSE_HEAD,
            HttpEventKind::ResponseData => HTTP_EVENT_RESPONSE_DATA,
            HttpEventKind::ResponseEnd => HTTP_EVENT_RESPONSE_END,
            HttpEventKind::Error => HTTP_EVENT_ERROR,
            HttpEventKind::Closed => HTTP_EVENT_CLOSED,
        };
        let (method, method_len) = event_bytes(event.method);
        let (target, target_len) = event_bytes(event.target);
        // The view's `reason` slot carries the status text of a response head and the
        // message of an error; the generated handler reads it by kind.
        let (reason, reason_len) = event_bytes(if event.kind == HttpEventKind::Error {
            event.message
        } else {
            event.reason
        });
        let (headers, headers_len) = event_bytes(event.headers);
        let (data, data_len) = event_bytes(if event.kind == HttpEventKind::Error {
            event.message
        } else {
            event.data
        });
        let view = HttpEventView {
            kind,
            flags: u32::from(event.keep_alive),
            id: event.id,
            status: event.status,
            version_minor: event.version_minor,
            reserved: 0,
            method,
            method_len,
            target,
            target_len,
            reason,
            reason_len,
            headers,
            headers_len,
            data,
            data_len,
        };
        // SAFETY: the engine keeps every span live for this synchronous call, and the
        // owner's adapter allocation lives until the connection is removed below.
        let status = unsafe { (self.owner.run)(self.owner.pointer, &raw const view, host, thrown) };
        if matches!(
            event.kind,
            HttpEventKind::Finish
                | HttpEventKind::ResponseEnd
                | HttpEventKind::Closed
                | HttpEventKind::Error
        ) {
            // The drive may be waiting for exactly this request to finish. A referenced host
            // request will wake it by completing; an unreferenced one -- a signal listener's or
            // an unref'd port's wait (realms issue 23) -- never may, and must not leave a
            // virtual-clock drive parked once nothing keeps the realm alive.
            let runtime = realm.runtime.state.borrow();
            if !runtime.has_referenced_host_work() {
                runtime.wake_drive();
            }
        }
        if event.kind == HttpEventKind::Closed {
            realm.http_connections.borrow_mut().remove(&self.resource);
            let _ = realm
                .runtime
                .state
                .borrow_mut()
                .retire_http_connection(self.resource);
        }
        status
    }

    fn checkpoint(&self) -> i32 {
        let Some(realm) = self.realm.upgrade() else {
            return INVALID_ARGUMENT;
        };
        if !realm_has_queued_work(&realm) {
            return 0;
        }
        let _online = reclaim::online();
        drain_realm_queues(&realm)
    }

    fn whole_requests(&self) -> bool {
        self.whole_requests
    }
}

/// `kiln_realm_http_server_attach_with_options_active`'s option bits. Bit 0 was the whole
/// of the argument (a boolean) before bit 1 existed, so every older caller still means what
/// it meant.
#[cfg(feature = "realm-http")]
const HTTP_ATTACH_ALLOW_HALF_CLOSE: i32 = 1;
#[cfg(feature = "realm-http")]
const HTTP_ATTACH_WHOLE_REQUESTS: i32 = 2;

#[cfg(feature = "realm-http")]
fn attach_http(
    resource: u64,
    role: HttpRole,
    allow_half_close: bool,
    whole_requests: bool,
    run: Option<RunHttpEvent>,
    drop: Option<DropWork>,
    work: *mut c_void,
) -> i32 {
    ffi(|| {
        let (Some(realm), Some(run), Some(drop)) = (active_realm(), run, drop) else {
            return INVALID_ARGUMENT;
        };
        if work.is_null() || resource == 0 {
            return INVALID_ARGUMENT;
        }
        let resource = kiln_host_abi::ResourceId::new(resource);
        if realm.http_connections.borrow().contains_key(&resource) {
            return INVALID_ARGUMENT;
        }
        let owner = Rc::new(HttpEventOwner {
            pointer: work,
            run,
            drop,
        });
        // ADR 0046: Hyper drives the socket; events call the C handler directly.
        let dispatch = Rc::new(NativeHttpDispatch {
            realm: Rc::downgrade(&realm),
            resource,
            owner: Rc::clone(&owner),
            whole_requests,
        });
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        realm.pin(AFFINITY_HTTP);
        let attached = match role {
            HttpRole::Server => runtime.server_attach(resource, allow_half_close, dispatch),
            HttpRole::Client => runtime.client_attach(resource, dispatch),
        };
        std::mem::drop(runtime);
        if !attached {
            return INVALID_ARGUMENT;
        }
        realm.http_connections.borrow_mut().insert(
            resource,
            NativeHttpConnection {
                role,
                events: owner,
            },
        );
        0
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_server_attach_active(
    resource: u64,
    run: Option<RunHttpEvent>,
    drop: Option<DropWork>,
    work: *mut c_void,
) -> i32 {
    attach_http(resource, HttpRole::Server, false, false, run, drop, work)
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_server_attach_with_options_active(
    resource: u64,
    options: i32,
    run: Option<RunHttpEvent>,
    drop: Option<DropWork>,
    work: *mut c_void,
) -> i32 {
    attach_http(
        resource,
        HttpRole::Server,
        options & HTTP_ATTACH_ALLOW_HALF_CLOSE != 0,
        options & HTTP_ATTACH_WHOLE_REQUESTS != 0,
        run,
        drop,
        work,
    )
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_client_attach_active(
    resource: u64,
    run: Option<RunHttpEvent>,
    drop: Option<DropWork>,
    work: *mut c_void,
) -> i32 {
    attach_http(resource, HttpRole::Client, false, false, run, drop, work)
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_client_request_id_active(out_id: *mut u64) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(out_id) = (unsafe { out_id.as_mut() }) else {
            return INVALID_ARGUMENT;
        };
        let id = realm.next_http_client_request_id.get();
        realm
            .next_http_client_request_id
            .set(id.wrapping_add(1).max(1));
        *out_id = id;
        0
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_agent_acquire_active(
    host: *const u8,
    host_len: u64,
    port: u16,
    request: u64,
    out_resource: *mut u64,
) -> i32 {
    ffi(|| {
        let (Some(realm), Some(out_resource)) = (active_realm(), unsafe { out_resource.as_mut() })
        else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        let Some(host) = (unsafe { borrowed_bytes(host, host_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Ok(host) = std::str::from_utf8(host) else {
            return INVALID_ARGUMENT;
        };
        *out_resource = 0;
        match runtime.acquire_client_connection(HttpOrigin::http(host, port), request) {
            AgentDecision::Connect => HTTP_AGENT_CONNECT,
            AgentDecision::Reuse(resource) => {
                *out_resource = resource.get();
                HTTP_AGENT_REUSE
            }
            AgentDecision::Queued => HTTP_AGENT_QUEUED,
        }
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_client_owner_active(
    resource: u64,
    out_owner: *mut *mut c_void,
) -> i32 {
    ffi(|| {
        let (Some(realm), Some(out_owner)) = (active_realm(), unsafe { out_owner.as_mut() }) else {
            return INVALID_ARGUMENT;
        };
        let resource = ResourceId::new(resource);
        let connections = realm.http_connections.borrow();
        let Some(connection) = connections.get(&resource) else {
            return INVALID_ARGUMENT;
        };
        if connection.role != HttpRole::Client {
            return INVALID_ARGUMENT;
        }
        *out_owner = connection.events.pointer;
        0
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_agent_connected_active(
    host: *const u8,
    host_len: u64,
    port: u16,
    resource: u64,
    request: u64,
) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        let Some(host) = (unsafe { borrowed_bytes(host, host_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Ok(host) = std::str::from_utf8(host) else {
            return INVALID_ARGUMENT;
        };
        i32::from(runtime.client_connection_connected(
            HttpOrigin::http(host, port),
            ResourceId::new(resource),
            request,
        ))
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_agent_release_active(
    resource: u64,
    reusable: i32,
    out_request: *mut u64,
) -> i32 {
    ffi(|| {
        let (Some(realm), Some(out_request)) = (active_realm(), unsafe { out_request.as_mut() })
        else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        *out_request = 0;
        match runtime.release_client_connection(ResourceId::new(resource), reusable != 0) {
            AgentRelease::Idle => HTTP_AGENT_IDLE,
            AgentRelease::Dispatch { request } => {
                *out_request = request;
                HTTP_AGENT_DISPATCH
            }
            AgentRelease::Retire => HTTP_AGENT_RETIRE,
        }
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_client_start_active(
    resource: u64,
    id: u64,
    request: *const HttpRequestView,
    high_water_mark: u64,
) -> i32 {
    ffi(|| {
        if request.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let resource = kiln_host_abi::ResourceId::new(resource);
        if realm
            .http_connections
            .borrow()
            .get(&resource)
            .is_none_or(|connection| connection.role != HttpRole::Client)
        {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the request frame and all spans are borrowed for this synchronous call.
        let request = unsafe { &*request };
        let Some(method) = (unsafe { borrowed_bytes(request.method, request.method_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(target) = (unsafe { borrowed_bytes(request.target, request.target_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(headers) = (unsafe { parse_headers(request.headers, request.headers_len) }) else {
            return INVALID_ARGUMENT;
        };
        let (Ok(method), Ok(target)) = (std::str::from_utf8(method), std::str::from_utf8(target))
        else {
            return INVALID_ARGUMENT;
        };
        let Ok(high_water_mark) = usize::try_from(high_water_mark) else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        let sent = runtime.http_client().send_request(
            resource,
            id,
            method,
            target,
            &headers,
            high_water_mark,
        );
        if sent { 0 } else { RUNTIME_ERROR }
    })
}

#[cfg(feature = "realm-http")]
fn write_http_client_request(
    resource: u64,
    id: u64,
    data: *const u8,
    data_len: u64,
    end: bool,
) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        // SAFETY: the generated adapter keeps this span live until this call copies it.
        let Some(data) = (unsafe { borrowed_bytes(data, data_len) }) else {
            return INVALID_ARGUMENT;
        };
        let client = runtime.http_client();
        let resource = kiln_host_abi::ResourceId::new(resource);
        let disposition = if end {
            client.end_request(resource, id, data)
        } else {
            client.write_request(resource, id, data)
        };
        match disposition {
            Some(disposition) => i32::from(!disposition.ready),
            None => RUNTIME_ERROR,
        }
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_client_write_active(
    resource: u64,
    id: u64,
    data: *const u8,
    data_len: u64,
) -> i32 {
    write_http_client_request(resource, id, data, data_len, false)
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_client_end_active(
    resource: u64,
    id: u64,
    data: *const u8,
    data_len: u64,
) -> i32 {
    write_http_client_request(resource, id, data, data_len, true)
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_response_start_active(
    resource: u64,
    id: u64,
    response: *const HttpResponseView,
    high_water_mark: u64,
) -> i32 {
    ffi(|| {
        if response.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        // SAFETY: this private frame remains live for the synchronous call.
        let response = unsafe { &*response };
        let Ok(high_water_mark) = usize::try_from(high_water_mark) else {
            return INVALID_ARGUMENT;
        };
        let resource = kiln_host_abi::ResourceId::new(resource);
        // The engine parses the flattened span itself, straight into Hyper's map.
        // SAFETY: the adapter keeps the span live for this synchronous call.
        let Some(headers) = (unsafe { borrowed_bytes(response.headers, response.headers_len) })
        else {
            return INVALID_ARGUMENT;
        };
        let started = runtime.http_server().start_response(
            resource,
            id,
            response.status,
            headers,
            high_water_mark,
        );
        if started { 0 } else { RUNTIME_ERROR }
    })
}

#[cfg(feature = "realm-http")]
fn write_http_response(resource: u64, id: u64, data: *const u8, data_len: u64, end: bool) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Some(mut runtime) = realm.runtime.state_mut() else {
            return RUNTIME_ERROR;
        };
        // SAFETY: the adapter keeps this borrowed span live until the command copies it.
        let Some(data) = (unsafe { borrowed_bytes(data, data_len) }) else {
            return INVALID_ARGUMENT;
        };
        let resource = kiln_host_abi::ResourceId::new(resource);
        let disposition = if end {
            runtime.http_server().end_response(resource, id, data)
        } else {
            runtime.http_server().write_response(resource, id, data)
        };
        match disposition {
            Some(disposition) => i32::from(!disposition.ready),
            None => RUNTIME_ERROR,
        }
    })
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_response_write_active(
    resource: u64,
    id: u64,
    data: *const u8,
    data_len: u64,
) -> i32 {
    write_http_response(resource, id, data, data_len, false)
}

#[cfg(feature = "realm-http")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_http_response_end_active(
    resource: u64,
    id: u64,
    data: *const u8,
    data_len: u64,
) -> i32 {
    write_http_response(resource, id, data, data_len, true)
}

/// Linkable HTTP stubs for archives built without `realm-http`.
///
/// Every symbol the provider ABI registry names must still link (PROVIDER_EXPORTS keeps
/// its 99 entries and PROVIDER_ABI_VERSION stays 37), so each entry point keeps its exact
/// signature and answers invalid-argument. The stubs are unreachable in an HTTP-free
/// graph: generated adapters only call them after a successful attach, and every attach
/// fails without the engines. Enforcement of that "no HTTP calls" invariant belongs to a
/// later packaging phase; nothing here can distinguish "HTTP stripped" from "no HTTP
/// used" at link time.
#[cfg(not(feature = "realm-http"))]
mod http_stubs {
    use super::{DropWork, HttpRequestView, HttpResponseView, INVALID_ARGUMENT, RunHttpEvent};
    use core::ffi::c_void;

    macro_rules! http_stub {
            ($name:ident ($($arg:ident : $ty:ty),*) -> i32) => {
                #[unsafe(no_mangle)]
                pub unsafe extern "C" fn $name($($arg: $ty),*) -> i32 {
                    $(let _ = $arg;)*
                    INVALID_ARGUMENT
                }
            };
        }

    http_stub!(kiln_realm_http_server_close_active(resource: u64, mode: i32) -> i32);
    http_stub!(kiln_realm_http_server_attach_active(resource: u64, run: Option<RunHttpEvent>, drop: Option<DropWork>, work: *mut c_void) -> i32);
    http_stub!(kiln_realm_http_server_attach_with_options_active(resource: u64, allow_half_close: i32, run: Option<RunHttpEvent>, drop: Option<DropWork>, work: *mut c_void) -> i32);
    http_stub!(kiln_realm_http_client_attach_active(resource: u64, run: Option<RunHttpEvent>, drop: Option<DropWork>, work: *mut c_void) -> i32);
    http_stub!(kiln_realm_http_client_request_id_active(out_id: *mut u64) -> i32);
    http_stub!(kiln_realm_http_agent_acquire_active(host: *const u8, host_len: u64, port: u16, request: u64, out_resource: *mut u64) -> i32);
    http_stub!(kiln_realm_http_client_owner_active(resource: u64, out_owner: *mut *mut c_void) -> i32);
    http_stub!(kiln_realm_http_agent_connected_active(host: *const u8, host_len: u64, port: u16, resource: u64, request: u64) -> i32);
    http_stub!(kiln_realm_http_agent_release_active(resource: u64, reusable: i32, out_request: *mut u64) -> i32);
    http_stub!(kiln_realm_http_client_start_active(resource: u64, id: u64, request: *const HttpRequestView, high_water_mark: u64) -> i32);
    http_stub!(kiln_realm_http_client_write_active(resource: u64, id: u64, data: *const u8, data_len: u64) -> i32);
    http_stub!(kiln_realm_http_client_end_active(resource: u64, id: u64, data: *const u8, data_len: u64) -> i32);
    http_stub!(kiln_realm_http_response_start_active(resource: u64, id: u64, response: *const HttpResponseView, high_water_mark: u64) -> i32);
    http_stub!(kiln_realm_http_response_write_active(resource: u64, id: u64, data: *const u8, data_len: u64) -> i32);
    http_stub!(kiln_realm_http_response_end_active(resource: u64, id: u64, data: *const u8, data_len: u64) -> i32);
}

#[cfg(not(feature = "realm-http"))]
pub use http_stubs::*;

/// What [`arm`] schedules, in one of the realm's three macrotask lanes (`kiln_runtime::Realm`).
#[derive(Clone, Copy)]
enum Arm {
    Timeout(f64),
    Interval(f64),
    Immediate,
    Io,
}

fn arm(
    arm: Arm,
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_id: *mut u64,
) -> i32 {
    ffi(|| {
        if out_id.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Ok(mut queue) = realm.work.try_borrow_mut() else {
            return RUNTIME_ERROR;
        };
        let Some(work) = NativeWork::new(work, run, drop, matches!(arm, Arm::Interval(_))) else {
            return INVALID_ARGUMENT;
        };
        // Node's clamp, and its warning on this process's stderr.
        let mut delay = |delay_ms: f64| {
            let (delay, warning) = queue.node_timer_delay(Some(delay_ms));
            if let Some(warning) = warning {
                use std::io::Write as _;
                let _ = std::io::stderr().write_all(warning.as_bytes());
            }
            delay
        };
        let id = match arm {
            Arm::Timeout(delay_ms) => {
                let delay = delay(delay_ms);
                queue.set_timeout(delay, work)
            }
            Arm::Interval(delay_ms) => {
                let delay = delay(delay_ms);
                queue.set_interval(delay, work)
            }
            Arm::Immediate => queue.set_immediate(work),
            Arm::Io => queue.queue_io(work),
        };
        std::mem::drop(queue);
        // A timer armed from inside a connection's callback (ADR 0046) changes what the
        // parked drive must wait for; tell it.
        if let Some(realm) = active_realm()
            && let Ok(state) = realm.runtime.state.try_borrow()
        {
            state.wake_drive();
        }
        // SAFETY: null was rejected and the caller supplies a writable identity slot.
        unsafe { out_id.write(id.get()) };
        0
    })
}

/// Schedules one opaque timeout on the active Realm.
///
/// # Safety
///
/// `out_id` must be writable and `work` must satisfy the callback ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_set_timeout(
    delay_ms: f64,
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_id: *mut u64,
) -> i32 {
    arm(Arm::Timeout(delay_ms), run, drop, work, out_id)
}

/// Schedules one opaque interval on the active Realm.
///
/// # Safety
///
/// `out_id` must be writable and `work` must satisfy the callback ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_set_interval(
    delay_ms: f64,
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_id: *mut u64,
) -> i32 {
    arm(Arm::Interval(delay_ms), run, drop, work, out_id)
}

/// Queues one opaque `setImmediate` callback on the active Realm: node's check phase.
///
/// # Safety
///
/// `out_id` must be writable and `work` must satisfy the callback ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_set_immediate(
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_id: *mut u64,
) -> i32 {
    arm(Arm::Immediate, run, drop, work, out_id)
}

/// Queues one opaque I/O callback on the active Realm: node's poll phase, where a message port
/// delivers (`__kilnQueueIo`).
///
/// # Safety
///
/// `out_id` must be writable and `work` must satisfy the callback ownership contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_queue_io(
    run: Option<RunWork>,
    drop: Option<DropWork>,
    work: *mut c_void,
    out_id: *mut u64,
) -> i32 {
    arm(Arm::Io, run, drop, work, out_id)
}

/// Cancels a timeout or interval on the active Realm. Unknown identities are a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_clear_timer(id: u64) -> i32 {
    ffi(|| {
        let Some(realm) = active_realm() else {
            return INVALID_ARGUMENT;
        };
        let Ok(mut queue) = realm.work.try_borrow_mut() else {
            return RUNTIME_ERROR;
        };
        queue.clear_timeout(TimerId::new(id));
        0
    })
}

/// Drives every queued microtask and timer on `handle` to deterministic quiescence.
///
/// # Safety
///
/// `host` and `thrown` must remain valid for every queued work callback during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_drive(
    handle: u64,
    host: *mut c_void,
    thrown: *mut c_void,
) -> i32 {
    ffi(|| {
        let Some(realm) = realm(handle) else {
            return INVALID_ARGUMENT;
        };
        let Some(_active) = ActiveRealm::enter(handle, Rc::clone(&realm)) else {
            return INVALID_ARGUMENT;
        };
        drive(&realm, host, thrown)
    })
}

/// Advance one scheduled language turn and return before waiting for host work. `kind` is
/// 0 ready, 1 parked for a task wake, 2 parked until `delay_ns`, or 3 done. The returned status
/// is meaningful for kind 3; otherwise it is zero. The scheduler sets the lane ticket waker
/// before entering this callback, and the launcher keeps both C slots alive until teardown.
///
/// # Safety
///
/// `host`, `thrown`, `kind` and `delay_ns` follow the same lifetime and writable-slot contracts
/// as `kiln_realm_drive` and the generated launcher's synchronous callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_drive_step(
    handle: u64,
    host: *mut c_void,
    thrown: *mut c_void,
    kind: *mut u32,
    delay_ns: *mut u64,
) -> i32 {
    ffi(|| {
        if kind.is_null() || delay_ns.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller supplied writable slots and null was rejected above.
        unsafe {
            kind.write(3);
            delay_ns.write(0);
        }
        let Some(realm) = realm(handle) else {
            return INVALID_ARGUMENT;
        };
        let Some(waker) = SCHEDULED_WAKER.with(|slot| slot.borrow().clone()) else {
            return INVALID_ARGUMENT;
        };
        let Some(_active) = ActiveRealm::enter(handle, Rc::clone(&realm)) else {
            return INVALID_ARGUMENT;
        };
        match drive_scheduled(&realm, host, thrown, &waker) {
            ScheduledStep::Ready => {
                unsafe { kind.write(0) };
                0
            }
            ScheduledStep::Parked => {
                unsafe { kind.write(1) };
                0
            }
            ScheduledStep::ParkedUntil(at) => {
                let nanos = at.saturating_duration_since(Instant::now()).as_nanos();
                unsafe {
                    kind.write(2);
                    delay_ns.write(nanos.min(u128::from(u64::MAX)) as u64);
                }
                0
            }
            ScheduledStep::Done(status) => status,
        }
    })
}

/// Destroys a Realm Runtime on its owning thread.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_realm_destroy(handle: u64) -> i32 {
    ffi(|| {
        if handle == 0 {
            return INVALID_ARGUMENT;
        }
        if ACTIVE_REALM.with(|active| active.get() == handle) {
            return INVALID_ARGUMENT;
        }
        // Teardown makes the realm's sites forget its local shapes, reading shared cache state.
        let _online = reclaim::online();
        REALMS.with(|realms| {
            if realms.borrow_mut().remove(&handle).is_some() {
                0
            } else {
                INVALID_ARGUMENT
            }
        })
    })
}

/// A launcher's body for one of several realms: `(context, index) -> status`. It creates, enters,
/// drives and destroys its realm itself, on the thread it is called on.
type PinnedRun = unsafe extern "C" fn(*mut c_void, u64) -> i32;

/// The launcher's context, shared by every realm thread; the launcher's body only reads it.
#[derive(Clone, Copy)]
struct SharedContext(*mut c_void);

// SAFETY: `kiln_realm_run_pinned`'s contract: `run` may be called with it from several threads.
unsafe impl Send for SharedContext {}

impl SharedContext {
    /// # Safety
    ///
    /// The launcher's callback contract permits concurrent calls with this context.
    unsafe fn begin(self, callback: ScheduledBegin, index: u64) -> *mut c_void {
        unsafe { callback(self.0, index) }
    }
}

type ScheduledBegin = unsafe extern "C" fn(*mut c_void, u64) -> *mut c_void;
type ScheduledIdentify = unsafe extern "C" fn(*mut c_void) -> u64;
type ScheduledAdvance = unsafe extern "C" fn(*mut c_void, *mut u64) -> i32;
type ScheduledFinish = unsafe extern "C" fn(*mut c_void) -> i32;

struct ScheduledRun {
    session: *mut c_void,
    realm: u64,
    detached: Option<DetachedRealm>,
    ticket: Option<Arc<kiln_runtime::turn_queue::TurnTicket>>,
    scheduler: Option<Weak<PinnedTurnScheduler>>,
    advance: ScheduledAdvance,
    finish: ScheduledFinish,
    result: std::sync::mpsc::Sender<(usize, i32)>,
    index: usize,
}

// SAFETY: a `ScheduledRun` is submitted to the movable scheduler only after its realm has
// detached its empty executor and sole registry reference, or after its ticket has pinned it to
// the constructing lane. At every later turn boundary it repeats that choice. The scheduler
// promotes a newly pinned body into the lane-private map before publishing its successor and
// destroys it on that lane. The generated session's C allocation has no live frame across a
// turn; only the exclusive ticket lease may call its callbacks.
unsafe impl Send for ScheduledRun {}

impl ScheduledRun {
    fn attach_if_detached(&mut self) {
        if let Some(detached) = self.detached.take() {
            attach_realm(self.realm, detached);
        }
    }

    fn park_ownership(&mut self, lane: usize) {
        let Some(ticket) = &self.ticket else {
            return;
        };
        if self.realm != 0 {
            self.detached = detach_realm(self.realm);
        }
        if self.detached.is_none() {
            // If cancellation raced this turn, `pin_to_lane` may reject a retired ticket; the
            // scheduler then drops this body on the running lane rather than publishing it.
            let _ = ticket.pin_to_lane(lane);
        }
    }

    fn complete(&mut self, override_status: Option<i32>) {
        self.attach_if_detached();
        let session = std::mem::replace(&mut self.session, std::ptr::null_mut());
        let status = if session.is_null() {
            RUNTIME_ERROR
        } else {
            // SAFETY: this lane owns the session's exclusive turn lease and no generated frame
            // remains live from its last advance, even if an earlier turn ran on another lane.
            unsafe { (self.finish)(session) }
        };
        let _ = self
            .result
            .send((self.index, override_status.unwrap_or(status)));
    }
}

impl PinnedTurn for ScheduledRun {
    fn run_turn(&mut self, waker: &Waker) -> TurnOutcome {
        self.run_turn_on(waker, 0)
    }

    fn run_turn_on(&mut self, waker: &Waker, lane: usize) -> TurnOutcome {
        let _pool = ScheduledPoolGuard::enter(self.scheduler.clone());
        if self.session.is_null() {
            self.complete(Some(RUNTIME_ERROR));
            return TurnOutcome::Done;
        }
        self.attach_if_detached();
        let _ticket = ScheduledWakerGuard::enter(waker.clone());
        let mut delay_ns = 0;
        // SAFETY: the ticket grants this lane exclusive access to the session, and the callback
        // returns with every generated C frame unwound. `delay_ns` is a writable result slot.
        let kind = unsafe { (self.advance)(self.session, &mut delay_ns) };
        match kind {
            0 => {
                self.complete(None);
                TurnOutcome::Done
            }
            1 => {
                self.park_ownership(lane);
                TurnOutcome::Ready
            }
            2 => {
                self.park_ownership(lane);
                TurnOutcome::Parked
            }
            3 => {
                self.park_ownership(lane);
                TurnOutcome::ParkedUntil(Instant::now() + Duration::from_nanos(delay_ns))
            }
            _ => {
                self.complete(Some(RUNTIME_ERROR));
                TurnOutcome::Done
            }
        }
    }
}

impl Drop for ScheduledRun {
    fn drop(&mut self) {
        if !self.session.is_null() {
            self.complete(Some(RUNTIME_ERROR));
        }
    }
}

/// Run many generated realm sessions over a fixed number of scheduler lanes. Each C callback
/// begins, advances or ends one session on its assigned lane; only its opaque context crosses
/// threads. Deterministic mode uses one FIFO lane and its own paused reactor.
///
/// # Safety
///
/// `begin` must be callable concurrently with `context`, `advance` and `finish` must operate
/// only on the session's owning lane, and `statuses` must hold `count` writable entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_run_scheduled(
    count: u64,
    lanes: u64,
    deterministic: i32,
    begin: Option<ScheduledBegin>,
    advance: Option<ScheduledAdvance>,
    finish: Option<ScheduledFinish>,
    context: *mut c_void,
    statuses: *mut i32,
) -> i32 {
    ffi(|| {
        let (Ok(count), Ok(lanes), Some(begin), Some(advance), Some(finish)) = (
            usize::try_from(count),
            usize::try_from(lanes),
            begin,
            advance,
            finish,
        ) else {
            return INVALID_ARGUMENT;
        };
        if count == 0 || lanes == 0 || statuses.is_null() {
            return INVALID_ARGUMENT;
        }
        let scheduler = if deterministic != 0 {
            PinnedTurnScheduler::deterministic()
        } else {
            PinnedTurnScheduler::new(lanes)
        };
        let Ok(scheduler) = scheduler else {
            return RUNTIME_ERROR;
        };
        let scheduler = Arc::new(scheduler);
        let weak_scheduler = Arc::downgrade(&scheduler);
        for index in 0..count {
            // SAFETY: caller supplied `count` writable status slots.
            unsafe { statuses.add(index).write(RUNTIME_ERROR) };
        }
        let shared = SharedContext(context);
        let (sender, receiver) = std::sync::mpsc::channel();
        let handles = (0..count)
            .map(|index| {
                let sender = sender.clone();
                let shared = shared;
                let weak_scheduler = weak_scheduler.clone();
                scheduler.spawn_pinned(move |reactor, _| {
                    let _reactor = ScheduledReactorGuard::enter(reactor);
                    let _pool = ScheduledPoolGuard::enter(Some(weak_scheduler.clone()));
                    // SAFETY: the caller's `begin`/context contract; this executes on the
                    // lane that will own the returned session and eventually call `finish`.
                    let session = unsafe { shared.begin(begin, index as u64) };
                    ScheduledRun {
                        session,
                        realm: 0,
                        detached: None,
                        ticket: None,
                        scheduler: Some(weak_scheduler),
                        advance,
                        finish,
                        result: sender,
                        index,
                    }
                })
            })
            .collect::<Vec<_>>();
        drop(sender);
        let mut received = 0;
        while received < count {
            let Ok((index, status)) = receiver.recv() else {
                break;
            };
            // SAFETY: `index` originated in the bounded spawn loop above.
            unsafe { statuses.add(index).write(status) };
            received += 1;
        }
        drop(handles);
        if received == count { 0 } else { RUNTIME_ERROR }
    })
}

/// Run generated sessions on scheduler lanes, moving an unpinned realm's exclusive turn lease
/// between lanes. `identify` names the realm created by `begin`; it may answer zero only for a
/// failed initialization. A realm that publishes thread-affine work pins its ticket permanently
/// before its next turn. The session's generated C frames return before any detach.
///
/// # Safety
///
/// `begin` and `context` permit concurrent calls; `identify`, `advance` and `finish` refer to
/// the same live session. All callbacks return with no retained C frame or borrowed lane pointer.
/// `statuses` holds `count` writable entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_run_migrating(
    count: u64,
    lanes: u64,
    deterministic: i32,
    begin: Option<ScheduledBegin>,
    identify: Option<ScheduledIdentify>,
    advance: Option<ScheduledAdvance>,
    finish: Option<ScheduledFinish>,
    context: *mut c_void,
    statuses: *mut i32,
) -> i32 {
    ffi(|| {
        let (Ok(count), Ok(lanes), Some(begin), Some(identify), Some(advance), Some(finish)) = (
            usize::try_from(count),
            usize::try_from(lanes),
            begin,
            identify,
            advance,
            finish,
        ) else {
            return INVALID_ARGUMENT;
        };
        if count == 0 || lanes == 0 || statuses.is_null() {
            return INVALID_ARGUMENT;
        }
        let scheduler = if deterministic != 0 {
            PinnedTurnScheduler::deterministic()
        } else {
            PinnedTurnScheduler::new(lanes)
        };
        let Ok(scheduler) = scheduler else {
            return RUNTIME_ERROR;
        };
        let scheduler = Arc::new(scheduler);
        let weak_scheduler = Arc::downgrade(&scheduler);
        for index in 0..count {
            // SAFETY: caller supplied `count` writable status slots.
            unsafe { statuses.add(index).write(RUNTIME_ERROR) };
        }
        let shared = SharedContext(context);
        let (sender, receiver) = std::sync::mpsc::channel();
        let handles = (0..count)
            .map(|index| {
                let sender = sender.clone();
                let shared = shared;
                let weak_scheduler = weak_scheduler.clone();
                scheduler.spawn_movable_with(move |reactor, ticket, lane| {
                    let _reactor = ScheduledReactorGuard::enter(reactor);
                    let _pool = ScheduledPoolGuard::enter(Some(weak_scheduler.clone()));
                    // SAFETY: the caller's callback contract; this first runs on a scheduler
                    // lane and returns with every generated frame unwound.
                    let session = unsafe { shared.begin(begin, index as u64) };
                    let realm = if session.is_null() {
                        0
                    } else {
                        // SAFETY: `begin` returned this live session.
                        unsafe { identify(session) }
                    };
                    let detached = (realm != 0).then(|| detach_realm(realm)).flatten();
                    if detached.is_none() {
                        let _ = ticket.pin_to_lane(lane);
                    }
                    ScheduledRun {
                        session,
                        realm,
                        detached,
                        ticket: Some(ticket),
                        scheduler: Some(weak_scheduler),
                        advance,
                        finish,
                        result: sender,
                        index,
                    }
                })
            })
            .collect::<Vec<_>>();
        drop(sender);
        let mut received = 0;
        while received < count {
            let Ok((index, status)) = receiver.recv() else {
                break;
            };
            // SAFETY: `index` originated in the bounded spawn loop above.
            unsafe { statuses.add(index).write(status) };
            received += 1;
        }
        drop(handles);
        if received == count { 0 } else { RUNTIME_ERROR }
    })
}

/// Runs `run(context, index)` for every index below `count` at once, each on its own realm thread
/// of one `WorkerPool` (realms issue 05: several pinned realms in one process), waits for all of
/// them, and writes each one's status to `statuses[index]`. A realm lives on one thread for its
/// whole life -- the provider's realm registry is per thread -- so `run` makes its realm there.
/// Returns 0 once every realm has finished, whatever their statuses; nonzero when a thread could
/// not be started (those realms' statuses say `RUNTIME_ERROR`).
///
/// # Safety
///
/// `run` must be callable from several threads at once with `context`, and `statuses` must have
/// room for `count` entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_run_pinned(
    count: u64,
    run: Option<PinnedRun>,
    context: *mut c_void,
    statuses: *mut i32,
) -> i32 {
    // SAFETY: the caller's contract, which is this function's.
    unsafe { run_realms(count, run, context, statuses, false) }
}

/// `kiln_realm_run_pinned` for `KILN_WORKERS` thread mode: the same N realm threads, each of
/// which first joins the process's listen group, so every realm listening on one address
/// shares one socket and the process balances accepted connections across them
/// (`kiln_runtime::listen_group`) instead of the kernel hashing them over N `SO_REUSEPORT`
/// sockets. Realms those realms start (`node:worker_threads`) are on other threads and listen
/// as they always have.
///
/// # Safety
///
/// As `kiln_realm_run_pinned`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_realm_run_workers(
    count: u64,
    run: Option<PinnedRun>,
    context: *mut c_void,
    statuses: *mut i32,
) -> i32 {
    // SAFETY: the caller's contract, which is this function's.
    unsafe { run_realms(count, run, context, statuses, true) }
}

/// # Safety
///
/// As `kiln_realm_run_pinned`.
unsafe fn run_realms(
    count: u64,
    run: Option<PinnedRun>,
    context: *mut c_void,
    statuses: *mut i32,
    listen_group: bool,
) -> i32 {
    ffi(|| {
        let (Some(run), Ok(count)) = (run, usize::try_from(count)) else {
            return INVALID_ARGUMENT;
        };
        if statuses.is_null() || count == 0 {
            return INVALID_ARGUMENT;
        }
        let pool = kiln_runtime::WorkerPool::<TokioHost>::new(count);
        let shared = SharedContext(context);
        let realms: Vec<_> = (0..count as u64)
            .map(|index| {
                // As deep a C stack as a main thread or a worker realm's (parallel issue 30):
                // compiled code recurses on it, up to the interpreter's 10,000 calls.
                pool.spawn_pinned_with_stack(kiln_runtime::workers::WORKER_STACK, move || {
                    let shared = shared;
                    if listen_group {
                        kiln_runtime::join_listen_group();
                        // Thread mode (realms issue 23): `process.exit` ends this realm and the
                        // launcher decides for the rest, rather than a C `exit` under the others.
                        kiln_runtime::workers::own_exits();
                    }
                    // SAFETY: the caller's contract for `run` and `context`.
                    let status = unsafe { run(shared.0, index) };
                    // A finished realm takes no more signals: one it listened for must not be
                    // swallowed on its behalf.
                    kiln_runtime::signals::release_thread();
                    status
                })
            })
            .collect();
        let mut started_all = true;
        for (index, realm) in realms.into_iter().enumerate() {
            let status = match realm {
                Ok(realm) => realm.join().unwrap_or(RUNTIME_ERROR),
                Err(_) => {
                    started_all = false;
                    RUNTIME_ERROR
                }
            };
            // SAFETY: the caller gave room for `count` statuses, and `index < count`.
            unsafe { statuses.add(index).write(status) };
        }
        if started_all { 0 } else { RUNTIME_ERROR }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "realm/tests.rs"]
mod tests;

// Keep the exported Rust declarations honest against the registry.  These are deliberately
// function-pointer assignments rather than symbol-name checks: changing an argument width,
// callback view, or return value now fails this crate's compilation.
macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (realm, kiln_realm_create, realm_create) => {
            const _: unsafe extern "C" fn(*mut u64) -> i32 = kiln_realm_create;
        };
        (realm, kiln_realm_enter, realm_enter) => {
            const _: unsafe extern "C" fn(u64, Option<Entry>, *mut c_void) -> i32 = kiln_realm_enter;
        };
        (realm, kiln_realm_is_active, realm_is_active) => {
            const _: extern "C" fn() -> i32 = kiln_realm_is_active;
        };
        (realm, kiln_realm_diagnostic_reset, realm_diagnostic_reset) => {
            const _: extern "C" fn() -> i32 = kiln_realm_diagnostic_reset;
        };
        (realm, kiln_realm_diagnostic_note, realm_diagnostic_note) => {
            const _: unsafe extern "C" fn(*const u8, u64, u64, *const u8, u64, u64, *const u8, u64, *const u8, u64, u64, u64, i32) -> i32 = kiln_realm_diagnostic_note;
        };
        (realm, kiln_realm_diagnostic_render, realm_diagnostic_render) => {
            const _: unsafe extern "C" fn(*mut u8, u64, *mut u64) -> i32 = kiln_realm_diagnostic_render;
        };
        (realm, kiln_node_path_part, node_path_part) => {
            const _: unsafe extern "C" fn(u32, *const u8, u64, *const u8, u64, *mut u8, u64, *mut u64) -> i32 = kiln_node_path_part;
        };
        (realm, kiln_node_path_relative, node_path_relative) => {
            const _: unsafe extern "C" fn(*const u8, u64, *const u8, u64, *mut u8, u64, *mut u64) -> i32 = kiln_node_path_relative;
        };
        (realm, kiln_url_parse, url_parse) => {
            const _: unsafe extern "C" fn(*const u8, u64, *const u8, u64, *mut u8, u64, *mut u64) -> i32 = kiln_url_parse;
        };
        (realm, kiln_url_set, url_set) => {
            const _: unsafe extern "C" fn(*const u8, u64, u32, *const u8, u64, *mut u8, u64, *mut u64) -> i32 = kiln_url_set;
        };
        (realm, kiln_string_case, string_case) => {
            const _: unsafe extern "C" fn(*const u16, u64, u32, *mut u16, u64, *mut u64) -> i32 = kiln_string_case;
        };
        (realm, kiln_realm_symbol_for, realm_symbol_for) => {
            const _: unsafe extern "C" fn(*const super::KilnString, *mut u64) -> i32 =
                kiln_realm_symbol_for;
        };
        (realm, kiln_realm_symbol_key_for, realm_symbol_key_for) => {
            const _: unsafe extern "C" fn(u64, *mut super::KilnString) -> i32 =
                kiln_realm_symbol_key_for;
        };
        (realm, kiln_realm_callable_store, realm_callable_store) => {
            const _: unsafe extern "C" fn(u32, u64, *mut c_void, Option<DropWork>, *mut *mut c_void) -> i32 = kiln_realm_callable_store;
        };
        (realm, kiln_realm_value_slot, realm_value_slot) => {
            const _: unsafe extern "C" fn(u32, *mut *mut c_void) -> i32 = kiln_realm_value_slot;
        };
        (realm, kiln_realm_set_argv, realm_set_argv) => {
            const _: unsafe extern "C" fn(u64, u64, *const *const c_char) -> i32 = kiln_realm_set_argv;
        };
        (realm, kiln_realm_argv_count, realm_argv_count) => {
            const _: unsafe extern "C" fn(*mut u64) -> i32 = kiln_realm_argv_count;
        };
        (realm, kiln_realm_argv_at, realm_argv_at) => {
            const _: unsafe extern "C" fn(u64, *mut *const u8, *mut u64) -> i32 = kiln_realm_argv_at;
        };
        (realm, kiln_realm_entropy_word, realm_entropy_word) => {
            const _: unsafe extern "C" fn(u64, *mut u64) -> i32 = kiln_realm_entropy_word;
        };
        (realm, kiln_realm_runtime_report, realm_runtime_report) => {
            const _: unsafe extern "C" fn(*mut u64, u64, *mut u64) -> i32 = kiln_realm_runtime_report;
        };
        (realm, kiln_realm_gc_current_heap, realm_gc_current_heap) => {
            const _: extern "C" fn() -> *mut c_void = kiln_realm_gc_current_heap;
        };
        (realm, kiln_realm_generation_slot, realm_generation_slot) => {
            const _: extern "C" fn() -> *const u64 = kiln_realm_generation_slot;
        };
        (realm, kiln_realm_process_shared, realm_process_shared) => {
            const _: extern "C" fn(u64) -> *mut c_void = kiln_realm_process_shared;
        };
        (realm, kiln_realm_retire_tag, realm_retire_tag) => {
            const _: extern "C" fn() -> u64 = reclaim::kiln_realm_retire_tag;
        };
        (realm, kiln_realm_retired_safe, realm_retired_safe) => {
            const _: extern "C" fn(u64) -> i32 = reclaim::kiln_realm_retired_safe;
        };
        (realm, kiln_realm_retire, realm_retire) => {
            const _: unsafe extern "C" fn(*mut c_void) -> i32 = reclaim::kiln_realm_retire;
        };
        (realm, kiln_shared_graph_retain, shared_graph_retain) => {
            const _: extern "C" fn(u32, u32, u32) -> u64 = shared_graph::kiln_shared_graph_retain;
        };
        (realm, kiln_shared_graph_release, shared_graph_release) => {
            const _: extern "C" fn(u64) -> i32 = shared_graph::kiln_shared_graph_release;
        };
        (realm, kiln_shared_graph_info, shared_graph_info) => {
            const _: unsafe extern "C" fn(u64, *mut shared_graph::SharedInfo) -> i32 = shared_graph::kiln_shared_graph_info;
        };
        (realm, kiln_shared_graph_field, shared_graph_field) => {
            const _: unsafe extern "C" fn(u64, u64, *mut shared_graph::SharedFieldInfo, *mut u16, u64, *mut u16, u64) -> i32 = shared_graph::kiln_shared_graph_field;
        };
        (realm, kiln_shared_graph_build, shared_graph_build) => {
            const _: unsafe extern "C" fn(u64, Option<shared_graph::SourceInspect>, Option<shared_graph::SourceUnit>, *mut c_void, *mut shared_graph::SharedBuildResult) -> i32 = shared_graph::kiln_shared_graph_build;
        };
        (realm, kiln_realm_run_pinned, realm_run_pinned) => {
            const _: unsafe extern "C" fn(u64, Option<PinnedRun>, *mut c_void, *mut i32) -> i32 = kiln_realm_run_pinned;
        };
        (realm, kiln_realm_run_workers, realm_run_pinned) => {
            const _: unsafe extern "C" fn(u64, Option<PinnedRun>, *mut c_void, *mut i32) -> i32 = kiln_realm_run_workers;
        };
        (realm, kiln_realm_run_scheduled, realm_run_scheduled) => {
            const _: unsafe extern "C" fn(u64, u64, i32, Option<ScheduledBegin>, Option<ScheduledAdvance>, Option<ScheduledFinish>, *mut c_void, *mut i32) -> i32 = kiln_realm_run_scheduled;
        };
        (realm, kiln_realm_run_migrating, realm_run_migrating) => {
            const _: unsafe extern "C" fn(u64, u64, i32, Option<ScheduledBegin>, Option<ScheduledIdentify>, Option<ScheduledAdvance>, Option<ScheduledFinish>, *mut c_void, *mut i32) -> i32 = kiln_realm_run_migrating;
        };
        (realm, kiln_realm_gc_attach, realm_gc_attach) => {
            const _: unsafe extern "C" fn(*const GcHooks, *mut c_void, *mut *mut c_void) -> i32 = kiln_realm_gc_attach;
        };
        (realm, kiln_realm_gc_register_roots, realm_gc_register_roots) => {
            const _: unsafe extern "C" fn(Option<GcRootsFn>, *mut c_void, u32) -> i32 = kiln_realm_gc_register_roots;
        };
        (realm, kiln_realm_gc_register_roots_with_visitor, realm_gc_register_roots_with_visitor) => {
            const _: unsafe extern "C" fn(Option<GcRootsFn>, Option<GcRootsVisitFn>, *mut c_void, u32) -> i32 = kiln_realm_gc_register_roots_with_visitor;
        };
        (realm, kiln_realm_gc_unregister_roots, realm_gc_unregister_roots) => {
            const _: unsafe extern "C" fn(Option<GcRootsFn>, *mut c_void) -> i32 = kiln_realm_gc_unregister_roots;
        };
        (realm, kiln_realm_gc_mark_roots, realm_gc_mark_roots) => {
            const _: unsafe extern "C" fn(*mut c_void) -> i32 = kiln_realm_gc_mark_roots;
        };
        (realm, kiln_realm_gc_visit_provider_roots, realm_gc_visit_provider_roots) => {
            const _: unsafe extern "C" fn(*mut c_void, Option<GcVisitRoot>, *mut c_void) -> i32 = kiln_realm_gc_visit_provider_roots;
        };
        (realm, kiln_realm_gc_mark_value, realm_gc_mark_value) => {
            const _: unsafe extern "C" fn(*mut c_void, *const c_void) -> i32 = kiln_realm_gc_mark_value;
        };
        (realm, kiln_realm_gc_mark_block, realm_gc_mark_block) => {
            const _: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32 = kiln_realm_gc_mark_block;
        };
        (realm, kiln_realm_now, realm_now) => {
            const _: unsafe extern "C" fn(*mut f64) -> i32 = kiln_realm_now;
        };
        (realm, kiln_realm_performance_now, realm_now) => {
            const _: unsafe extern "C" fn(*mut f64) -> i32 = kiln_realm_performance_now;
        };
        (realm, kiln_realm_now_nanoseconds, realm_now_nanoseconds) => {
            const _: unsafe extern "C" fn(*mut i64, *mut u64) -> i32 = kiln_realm_now_nanoseconds;
        };
        (realm, kiln_realm_set_promise_lane, realm_set_promise_lane) => {
            const _: unsafe extern "C" fn(
                Option<RunPromiseLane>,
                Option<PromiseLanePending>,
                *mut c_void,
            ) -> i32 = kiln_realm_set_promise_lane;
        };
        (realm, $name:ident, realm_queue_work) => {
            const _: unsafe extern "C" fn(Option<RunWork>, Option<DropWork>, *mut c_void) -> i32 = $name;
        };
        (realm, $name:ident, realm_set_timer) => {
            const _: unsafe extern "C" fn(f64, Option<RunWork>, Option<DropWork>, *mut c_void, *mut u64) -> i32 = $name;
        };
        (realm, $name:ident, realm_queue_macrotask) => {
            const _: unsafe extern "C" fn(Option<RunWork>, Option<DropWork>, *mut c_void, *mut u64) -> i32 = $name;
        };
        (realm, kiln_realm_clear_timer, realm_clear_timer) => {
            const _: extern "C" fn(u64) -> i32 = kiln_realm_clear_timer;
        };
        (realm, kiln_realm_drive, realm_drive) => {
            const _: unsafe extern "C" fn(u64, *mut c_void, *mut c_void) -> i32 = kiln_realm_drive;
        };
        (realm, kiln_realm_drive_step, realm_drive_step) => {
            const _: unsafe extern "C" fn(u64, *mut c_void, *mut c_void, *mut u32, *mut u64) -> i32 = kiln_realm_drive_step;
        };
        (realm, kiln_realm_destroy, realm_destroy) => {
            const _: extern "C" fn(u64) -> i32 = kiln_realm_destroy;
        };
        (realm, kiln_realm_read_file, realm_read_file) => {
            const _: unsafe extern "C" fn(u64, *const u8, u64, Option<RunCompletion>, Option<DropWork>, *mut c_void, *mut u64) -> i32 = kiln_realm_read_file;
        };
        (realm, kiln_realm_start_host_op, realm_start_host_op) => {
            const _: unsafe extern "C" fn(u64, *const HostOpView, Option<RunCompletion>, Option<DropWork>, *mut c_void, *mut u64) -> i32 = kiln_realm_start_host_op;
        };
        (realm, kiln_realm_start_active_host_op, realm_start_active_host_op) => {
            const _: unsafe extern "C" fn(*const HostOpView, Option<RunCompletion>, Option<DropWork>, *mut c_void, *mut u64) -> i32 = kiln_realm_start_active_host_op;
        };
        (realm, kiln_realm_cancel_request, realm_cancel_request) => {
            const _: unsafe extern "C" fn(u64, u64) -> i32 = kiln_realm_cancel_request;
        };
        (realm, kiln_realm_cancel_active_request, realm_cancel_active_request) => {
            const _: unsafe extern "C" fn(u64) -> i32 = kiln_realm_cancel_active_request;
        };
        (realm, $name:ident, realm_http_attach) => {
            const _: unsafe extern "C" fn(u64, Option<RunHttpEvent>, Option<DropWork>, *mut c_void) -> i32 = $name;
        };
        (realm, kiln_realm_http_server_attach_with_options_active, realm_http_attach_options) => {
            const _: unsafe extern "C" fn(u64, i32, Option<RunHttpEvent>, Option<DropWork>, *mut c_void) -> i32 = kiln_realm_http_server_attach_with_options_active;
        };
        (realm, kiln_realm_http_server_close_active, realm_http_close_mode) => {
            const _: unsafe extern "C" fn(u64, i32) -> i32 = kiln_realm_http_server_close_active;
        };
        (realm, kiln_realm_http_client_request_id_active, realm_http_request_id) => {
            const _: unsafe extern "C" fn(*mut u64) -> i32 = kiln_realm_http_client_request_id_active;
        };
        (realm, kiln_realm_http_agent_acquire_active, realm_http_agent_acquire) => {
            const _: unsafe extern "C" fn(*const u8, u64, u16, u64, *mut u64) -> i32 = kiln_realm_http_agent_acquire_active;
        };
        (realm, kiln_realm_http_client_owner_active, realm_http_client_owner) => {
            const _: unsafe extern "C" fn(u64, *mut *mut c_void) -> i32 = kiln_realm_http_client_owner_active;
        };
        (realm, kiln_realm_http_agent_connected_active, realm_http_agent_connected) => {
            const _: unsafe extern "C" fn(*const u8, u64, u16, u64, u64) -> i32 = kiln_realm_http_agent_connected_active;
        };
        (realm, kiln_realm_http_agent_release_active, realm_http_agent_release) => {
            const _: unsafe extern "C" fn(u64, i32, *mut u64) -> i32 = kiln_realm_http_agent_release_active;
        };
        (realm, kiln_realm_http_client_start_active, realm_http_client_start) => {
            const _: unsafe extern "C" fn(u64, u64, *const HttpRequestView, u64) -> i32 = kiln_realm_http_client_start_active;
        };
        (realm, $name:ident, realm_http_client_write) => {
            const _: unsafe extern "C" fn(u64, u64, *const u8, u64) -> i32 = $name;
        };
        (realm, kiln_realm_http_response_start_active, realm_http_response_start) => {
            const _: unsafe extern "C" fn(u64, u64, *const HttpResponseView, u64) -> i32 = kiln_realm_http_response_start_active;
        };
        (realm, $name:ident, realm_http_response_write) => {
            const _: unsafe extern "C" fn(u64, u64, *const u8, u64) -> i32 = $name;
        };
        (realm, kiln_node_fs_read_file_sync, node_fs_read_file_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut *mut u8, *mut u64, *mut i32) -> i32 = kiln_node_fs_read_file_sync;
        };
        (realm, kiln_node_fs_free_bytes, node_fs_free_bytes) => {
            const _: unsafe extern "C" fn(*mut u8, u64) = kiln_node_fs_free_bytes;
        };
        (realm, kiln_node_fs_write_file_sync, node_fs_write_file_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, *const u8, u64, *mut i32) -> i32 = kiln_node_fs_write_file_sync;
        };
        (realm, kiln_node_fs_mkdir_sync, node_fs_mkdir_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, i32, *mut i32) -> i32 = kiln_node_fs_mkdir_sync;
        };
        (realm, kiln_node_fs_stat_sync, node_fs_stat_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, i32, *mut u64, *mut u32, *mut f64, *mut u32, *mut i32) -> i32 = kiln_node_fs_stat_sync;
        };
        (realm, kiln_node_fs_readdir_sync, node_fs_readdir_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut *mut u8, *mut u64, *mut u64, *mut i32) -> i32 = kiln_node_fs_readdir_sync;
        };
        (realm, kiln_node_fs_realpath_sync, node_fs_realpath_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut *mut u8, *mut u64, *mut i32) -> i32 = kiln_node_fs_realpath_sync;
        };
        (realm, kiln_node_fs_unlink_sync, node_fs_unlink_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut i32) -> i32 = kiln_node_fs_unlink_sync;
        };
        (realm, kiln_node_fs_rmdir_sync, node_fs_rmdir_sync) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut i32) -> i32 = kiln_node_fs_rmdir_sync;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

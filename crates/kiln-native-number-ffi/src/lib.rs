//! Private C ABI adapter for the semantics a generated artifact cannot compute for itself.
//!
//! Three of those so far: ECMAScript number conversion, whose shortest spelling no printf
//! conversion can express; regular expressions, which are an engine rather than a function; and
//! arbitrary-precision integers, which are exactly specified and still no easier to get right
//! twice (ADR 0025). All three are here for the same reason -- one implementation, shared with the
//! interpreter, rather than two that agree until they do not.
//!
//! The exported names are link-time implementation details of generated artifacts, not additions
//! to the public embedding ABI.
//!
//! The crate is still called `kiln-native-number-ffi`, which is now half a name. Renaming it moves
//! a packaging contract -- the manifest field, the archive filename, the environment variable --
//! and that is `kiln-native`'s to move; see NOTES.md.
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

#[cfg(any(feature = "regexp", feature = "bigint", feature = "json"))]
extern crate alloc;

const INVALID_ARGUMENT: i32 = -1;
#[cfg(feature = "realm")]
const RUNTIME_ERROR: i32 = -8;

/// The private native Realm ABI.
///
/// Handles are process-unique integers rather than Rust pointers. The owning thread keeps the
/// actual runtime in thread-local storage, so a stale or cross-thread handle is rejected without
/// dereferencing caller-controlled memory. No Tokio or Rust layout crosses the C seam.
#[cfg(feature = "realm")]
mod realm {
    use super::{INVALID_ARGUMENT, RUNTIME_ERROR};
    use core::ffi::{CStr, c_char, c_void};
    use kiln_host_abi::{
        AsyncHostError, AsyncHostOp, AsyncHostValue, RequestId, ResourceId, ResourceKind,
    };
    use kiln_runtime::{
        ActiveHttpRuntime, HttpCloseMode, Realm, RealmRuntime, RuntimeTurn, TimerId, WorkItem,
        duration_from_milliseconds,
        node_http_agent::{AgentDecision, AgentRelease, HttpOrigin},
    };
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

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

    const HTTP_EVENT_REQUEST_HEAD: u32 = 1;
    const HTTP_EVENT_REQUEST_DATA: u32 = 2;
    const HTTP_EVENT_REQUEST_END: u32 = 3;
    const HTTP_EVENT_DRAIN: u32 = 4;
    const HTTP_EVENT_FINISH: u32 = 5;
    const HTTP_EVENT_RESPONSE_HEAD: u32 = 6;
    const HTTP_EVENT_RESPONSE_DATA: u32 = 7;
    const HTTP_EVENT_RESPONSE_END: u32 = 8;
    const HTTP_EVENT_ERROR: u32 = 9;
    const HTTP_EVENT_CLOSED: u32 = 10;
    const HTTP_AGENT_CONNECT: i32 = 1;
    const HTTP_AGENT_REUSE: i32 = 2;
    const HTTP_AGENT_QUEUED: i32 = 3;
    const HTTP_AGENT_IDLE: i32 = 4;
    const HTTP_AGENT_DISPATCH: i32 = 5;
    const HTTP_AGENT_RETIRE: i32 = 6;
    const HTTP_CLOSE_GRACEFUL: i32 = 0;
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

    const VALUE_UNDEFINED: u32 = 0;
    const VALUE_BYTES: u32 = 1;
    const VALUE_FILE_STAT: u32 = 2;
    const VALUE_RESOURCE: u32 = 3;
    const VALUE_RESOURCE_STATE: u32 = 4;
    const VALUE_ACCEPTED_STREAM: u32 = 5;

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

    #[derive(Clone)]
    struct NativeWork(Rc<WorkOwner>);

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

        fn mark_roots(&self, marker: &GcMarker) {
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
            Some(Self(Rc::new(WorkOwner {
                pointer,
                run,
                drop,
                roots,
            })))
        }

        fn mark_roots(&self, marker: &GcMarker) {
            match self.0.roots {
                WorkRoots::Block => marker.block(self.0.pointer),
                WorkRoots::Completion => {
                    // SAFETY: a Completion work item is always the boxed CompletionWork that
                    // `drive` queued, and it stays boxed until its drop callback runs.
                    let work = unsafe { &*self.0.pointer.cast::<CompletionWork>() };
                    if let Some(continuation) = &work.continuation {
                        marker.block(continuation.pointer);
                    }
                }
            }
        }

        fn run(&self, host: *mut c_void, thrown: *mut c_void) -> i32 {
            // SAFETY: the C Adapter supplied this callback with the allocation, and the caller of
            // `kiln_realm_drive` keeps `host` and `thrown` live for the complete drive.
            unsafe { (self.0.run)(self.0.pointer, host, thrown) }
        }
    }

    struct AsyncContinuation {
        pointer: *mut c_void,
        run: RunCompletion,
        drop: DropWork,
    }

    struct HttpEventOwner {
        pointer: *mut c_void,
        run: RunHttpEvent,
        drop: DropWork,
    }

    impl Drop for HttpEventOwner {
        fn drop(&mut self) {
            // SAFETY: attach transfers one live adapter allocation and its matching disposer.
            unsafe { (self.drop)(self.pointer) };
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum HttpRole {
        Server,
        Client,
    }

    struct NativeHttpConnection {
        role: HttpRole,
        events: Rc<HttpEventOwner>,
    }

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

    struct NativeRealm {
        runtime: RefCell<RealmRuntime>,
        work: RefCell<Realm<NativeMicrotask, NativeWork>>,
        continuations: RefCell<HashMap<RequestId, AsyncContinuation>>,
        http_connections: RefCell<HashMap<kiln_host_abi::ResourceId, NativeHttpConnection>>,
        next_http_client_request_id: Cell<u64>,
        cancelled: Rc<RefCell<HashSet<RequestId>>>,
        queued: Rc<RefCell<HashSet<RequestId>>>,
        symbols: RefCell<HashMap<Vec<u8>, u64>>,
        symbol_keys: RefCell<HashMap<u64, Vec<u8>>>,
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
    const GC_ROOT_SURVIVES_TEARDOWN: u32 = 1;

    struct GcRoot {
        roots: GcRootsFn,
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

    impl GcMarker {
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

    /// Marks everything the archive keeps a JavaScript reference to, then the roots each
    /// translation unit and the launcher registered.
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
        for slot in realm.value_slots.borrow().values() {
            marker.value(slot.as_ptr().cast::<c_void>());
        }
        for store in realm.callable_stores.borrow().values() {
            marker.block(store.pointer);
        }
        for continuation in realm.continuations.borrow().values() {
            marker.block(continuation.pointer);
        }
        for connection in realm.http_connections.borrow().values() {
            marker.block(connection.events.pointer);
        }
        {
            let work = realm.work.borrow();
            work.for_each_queued(|item| item.mark_roots(&marker));
            work.for_each_timer(|item| item.mark_roots(&marker));
        }
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

    impl Drop for NativeRealm {
        fn drop(&mut self) {
            // Release what the archive holds first, while the heap is still alive: a queued work
            // item or a continuation drops through a C callback that may release its block. Then
            // mark only the roots that must outlive the realm (a borrowed result slot) and free
            // the rest, and finally let every unit reset the statics that pointed into the heap.
            *self.work.borrow_mut() = Realm::new();
            self.continuations.borrow_mut().clear();
            self.http_connections.borrow_mut().clear();
            self.callable_stores.borrow_mut().clear();
            self.value_slots.borrow_mut().clear();
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
        fn new() -> Result<Self, kiln_runtime::RuntimeBuildError> {
            Ok(Self {
                runtime: RefCell::new(RealmRuntime::new()?),
                work: RefCell::new(Realm::new()),
                continuations: RefCell::new(HashMap::new()),
                http_connections: RefCell::new(HashMap::new()),
                next_http_client_request_id: Cell::new(1),
                cancelled: Rc::new(RefCell::new(HashSet::new())),
                queued: Rc::new(RefCell::new(HashSet::new())),
                symbols: RefCell::new(HashMap::new()),
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
                gc: RefCell::new(GcState::default()),
            })
        }
    }

    struct ActiveRealm {
        realm: Rc<NativeRealm>,
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
            Some(Self { realm })
        }
    }

    impl Drop for ActiveRealm {
        fn drop(&mut self) {
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
        static ACTIVE_RUNTIME: Cell<*mut RealmRuntime> = const { Cell::new(std::ptr::null_mut()) };
        /// Bumped whenever the answer to "which realm, which heap" changes on this thread: a
        /// realm entered or left, a heap attached. The prelude caches per-realm answers in
        /// thread-locals of its own and compares against this before trusting one, reading it
        /// through the pointer `kiln_realm_generation_slot` hands out, which is stable for the
        /// thread's life. A generation rather than the heap pointer itself, because a heap
        /// address can be freed and reused by the next realm on the same thread.
        static ACTIVE_GENERATION: Cell<u64> = const { Cell::new(1) };
    }

    fn bump_generation() {
        ACTIVE_GENERATION.with(|generation| generation.set(generation.get().wrapping_add(1)));
    }

    /// The address of this thread's realm generation, for the prelude to read directly.
    #[unsafe(no_mangle)]
    pub extern "C" fn kiln_realm_generation_slot() -> *const u64 {
        ACTIVE_GENERATION.with(|generation| generation.as_ptr() as *const u64)
    }

    struct ActiveRuntime {
        previous: *mut RealmRuntime,
    }

    impl ActiveRuntime {
        fn enter(runtime: *mut RealmRuntime) -> Self {
            let previous = ACTIVE_RUNTIME.with(|active| active.replace(runtime));
            Self { previous }
        }
    }

    impl Drop for ActiveRuntime {
        fn drop(&mut self) {
            ACTIVE_RUNTIME.with(|active| active.set(self.previous));
        }
    }

    fn ffi(operation: impl FnOnce() -> i32) -> i32 {
        catch_unwind(AssertUnwindSafe(operation)).unwrap_or(RUNTIME_ERROR)
    }

    fn realm(handle: u64) -> Option<Rc<NativeRealm>> {
        if handle == 0 {
            return None;
        }
        REALMS.with(|realms| realms.borrow().get(&handle).cloned())
    }

    fn active_realm() -> Option<Rc<NativeRealm>> {
        ACTIVE_REALM_RC.with(|slot| slot.borrow().clone())
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

    fn active_runtime() -> Option<*mut RealmRuntime> {
        ACTIVE_RUNTIME.with(|active| {
            let pointer = active.get();
            if pointer.is_null() {
                None
            } else {
                Some(pointer)
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
            _ => return None,
        })
    }

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

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_server_close_active(resource: u64, mode: i32) -> i32 {
        ffi(|| {
            let (Some(_realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            let mode = match mode {
                HTTP_CLOSE_GRACEFUL => HttpCloseMode::Graceful,
                HTTP_CLOSE_ABORT => HttpCloseMode::Abort,
                _ => return INVALID_ARGUMENT,
            };
            // SAFETY: this private ABI call runs synchronously inside the active realm.
            let runtime = unsafe { &mut *runtime };
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
        let executor = realm.runtime.borrow().drive_executor();
        let _active = ActiveRuntime::enter(realm.runtime.as_ptr());
        realm.drive_context.set((host, thrown));
        let status = executor.block_on(drive_async(realm));
        realm
            .drive_context
            .set((std::ptr::null_mut(), std::ptr::null_mut()));
        status
    }

    async fn drive_async(realm: &NativeRealm) -> i32 {
        loop {
            tick_clock(realm);
            collect_if_due(realm);
            introspect_if_requested(realm);
            let status = drain_realm_queues(realm);
            if status != 0 {
                return status;
            }
            let turn = {
                let mut runtime = realm.runtime.borrow_mut();
                if matches!(realm.clock.get(), NativeClock::Virtual) {
                    runtime.pause_time();
                }
                runtime.ready_turn::<NativeMicrotask, NativeWork>()
            };
            match turn {
                Some(RuntimeTurn::Completion(completion)) => {
                    if let Some(status) = route_completion(realm, completion) {
                        return status;
                    }
                    while let Some(RuntimeTurn::Completion(next)) = {
                        let mut runtime = realm.runtime.borrow_mut();
                        runtime.ready_turn::<NativeMicrotask, NativeWork>()
                    } {
                        if let Some(status) = route_completion(realm, next) {
                            return status;
                        }
                    }
                    continue;
                }
                Some(RuntimeTurn::Failure(_)) => return RUNTIME_ERROR,
                Some(RuntimeTurn::Timer(WorkItem::Timer { work, .. })) => {
                    let (host, thrown) = realm.drive_context.get();
                    let status = work.run(host, thrown);
                    if status != 0 {
                        return status;
                    }
                    continue;
                }
                Some(RuntimeTurn::Timer(WorkItem::Microtask(_))) => {
                    unreachable!("the native drive drains microtasks before selecting a turn")
                }
                // The native dispatch calls the language directly; nothing is ever queued for
                // this drive, so a ready turn never reports it.
                Some(RuntimeTurn::Protocol) | None => {}
            }
            let (referenced, pending, in_flight, deadline, now, signal) = {
                let runtime = realm.runtime.borrow();
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
            if kiln_runtime::http_engine::trace_enabled() {
                eprintln!(
                    "[kiln-drive] referenced={referenced} pending={pending} in_flight={in_flight} deadline={deadline:?} host={}",
                    realm.runtime.borrow().describe_host_work(),
                );
            }
            // A timer that is already due runs now. It is not a wait case: under load the
            // completion signal always has a permit, and a select that prefers it would let a
            // due timer wait forever (measured: setTimeout(0) at 13 ms median on a saturated
            // server while every other route took 0.13 ms).
            if let Some(due) = deadline
                && due <= now
            {
                let timer = realm.work.borrow_mut().run_next_timer();
                if let Some(WorkItem::Timer { work, .. }) = timer {
                    let (host, thrown) = realm.drive_context.get();
                    let status = work.run(host, thrown);
                    if status != 0 {
                        return status;
                    }
                }
                continue;
            }
            // A completion can arrive only while an operation is pending, and it matters only
            // while something referenced keeps the realm alive; a direct connection with a
            // response outstanding is alive on its own. Otherwise only virtual timers remain.
            let completions_possible = referenced && pending;
            if !completions_possible && in_flight == 0 {
                let Some(deadline) = deadline else {
                    return 0;
                };
                // A wall-clock realm (ADR 0044) waits the real interval; the next turn finds the
                // timer due. A virtual realm jumps its clock to the deadline instead.
                if matches!(realm.clock.get(), NativeClock::Wall { .. }) {
                    let Some(signal) = signal else {
                        return RUNTIME_ERROR;
                    };
                    let delay = introspect_park(realm, Some(deadline.saturating_sub(now)));
                    kiln_runtime::wait_for_turn(signal, delay).await;
                    continue;
                }
                let timer = realm.work.borrow_mut().run_next_timer();
                if let Some(WorkItem::Timer { work, .. }) = timer {
                    let (host, thrown) = realm.drive_context.get();
                    let status = work.run(host, thrown);
                    if status != 0 {
                        return status;
                    }
                }
                continue;
            }
            let Some(signal) = signal else {
                return RUNTIME_ERROR;
            };
            // Yield first: a task woken by the last turn (a Hyper connection with a request to
            // dispatch, a completion about to land) runs now, before anything parks.
            kiln_runtime::yield_to_tasks().await;
            let delay =
                introspect_park(realm, deadline.map(|deadline| deadline.saturating_sub(now)));
            if kiln_runtime::wait_for_turn(signal, delay).await == kiln_runtime::DriveWake::Timer {
                let timer = realm.work.borrow_mut().run_next_timer();
                if let Some(WorkItem::Timer { work, .. }) = timer {
                    let (host, thrown) = realm.drive_context.get();
                    let status = work.run(host, thrown);
                    if status != 0 {
                        return status;
                    }
                }
            }
        }
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
        let payload = CompletionPayload::from_result(
            completion.request,
            completion.sequence,
            completion.result,
        );
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
            let Ok(realm) = NativeRealm::new() else {
                return RUNTIME_ERROR;
            };
            let Ok(handle) =
                NEXT_REALM.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                })
            else {
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
            let _active_runtime = ActiveRuntime::enter(realm.runtime.as_ptr());
            // SAFETY: the C caller owns `context` and the callback contract for this call.
            unsafe { &mut *realm.runtime.as_ptr() }.enter(|| unsafe { entry(context) })
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
                rendered.extend_from_slice(
                    format!(" receiver_tag={}", diagnostic.receiver_tag).as_bytes(),
                );
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

    /// Interns a registered-symbol key in the active Realm. The returned identity is stable for
    /// that Realm and deliberately meaningless outside it; generated modules never own registry
    /// state independently.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_symbol_for(
        key: *const u8,
        key_len: u64,
        out_id: *mut u64,
    ) -> i32 {
        ffi(|| {
            if out_id.is_null() {
                return INVALID_ARGUMENT;
            }
            let Some(key) = (unsafe { borrowed_bytes(key, key_len) }) else {
                return INVALID_ARGUMENT;
            };
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            if let Some(id) = realm.symbols.borrow().get(key).copied() {
                unsafe { out_id.write(id) };
                return 0;
            }
            let Ok(id) = NEXT_SYMBOL.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            }) else {
                return RUNTIME_ERROR;
            };
            let owned = key.to_vec();
            realm.symbols.borrow_mut().insert(owned.clone(), id);
            realm.symbol_keys.borrow_mut().insert(id, owned);
            unsafe { out_id.write(id) };
            0
        })
    }

    /// Borrows a registered-symbol key from the active Realm for the duration of this call.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_symbol_key_for(
        id: u64,
        out_key: *mut *const u8,
        out_key_len: *mut u64,
    ) -> i32 {
        ffi(|| {
            if out_key.is_null() || out_key_len.is_null() {
                return INVALID_ARGUMENT;
            }
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let keys = realm.symbol_keys.borrow();
            let Some(key) = keys.get(&id) else {
                return INVALID_ARGUMENT;
            };
            unsafe {
                out_key.write(key.as_ptr());
                out_key_len.write(key.len() as u64);
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
            let (Some(_realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: the drive installed this pointer for the realm it is driving, and the
            // turn boundary this is called from holds no borrow of it.
            let runtime = unsafe { &mut *runtime };
            let metrics = runtime.sample_runtime_metrics();
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
        ffi(|| {
            let Some(roots) = roots else {
                return INVALID_ARGUMENT;
            };
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let mut gc = realm.gc.borrow_mut();
            if gc
                .roots
                .iter()
                .any(|root| root.roots as usize == roots as usize && root.context == context)
            {
                return 0;
            }
            if gc.roots.try_reserve(1).is_err() {
                return RUNTIME_ERROR;
            }
            gc.roots.push(GcRoot {
                roots,
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

    /// Marks one value slot through `heap`'s hooks, for root functions outside the prelude.
    ///
    /// # Safety
    ///
    /// `heap` must be a live attached heap and `value` a readable `kiln_value`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_gc_mark_value(
        heap: *mut c_void,
        value: *const c_void,
    ) -> i32 {
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
    pub unsafe extern "C" fn kiln_realm_gc_mark_block(
        heap: *mut c_void,
        pointer: *mut c_void,
    ) -> i32 {
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
    pub unsafe extern "C" fn kiln_realm_now_nanoseconds(
        out_high: *mut i64,
        out_low: *mut u64,
    ) -> i32 {
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
        unsafe {
            kiln_realm_start_host_op(handle, &raw const operation, run, drop, work, out_request)
        }
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
            if let AsyncHostOp::CloseResource { resource } = &operation
                && realm.http_connections.borrow().contains_key(resource)
            {
                // Once the realm has attached Hyper, protocol shutdown owns this TCP resource.
                // A raw HostOps close can discard bytes Hyper has selected but not yet submitted,
                // which is exactly the authority split ADR 0032 forbids. Generated adapters must
                // request graceful/abort HTTP close through the dedicated realm operation.
                return INVALID_ARGUMENT;
            }
            let Some(runtime) = active_runtime() else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: `runtime` is the active realm's runtime, and the callback runs synchronously
            // while the runtime's owned Tokio task is executing.
            // `flags & 1` is the generic request-liveness bit; zero leaves incidental inspection
            // requests unreferenced while resource creation still records its own resource state.
            let referenced = operation_view.flags & 1 != 0;
            if kiln_runtime::http_engine::trace_enabled() {
                eprintln!("[kiln-host] start {operation:?} referenced={referenced}");
            }
            let request = unsafe { &mut *runtime }.start_host_op(operation, referenced);
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
            let Some(runtime) = active_runtime() else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: `runtime` is the active realm's runtime for this synchronous callback.
            let canceled = unsafe { &mut *runtime }.cancel_host_op(request);
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
    fn realm_has_queued_work(realm: &NativeRealm) -> bool {
        if realm.work.borrow().has_queued_work() {
            return true;
        }
        realm.promise_lane.get().is_some_and(PromiseLane::pending)
    }

    /// The realm's side of a direct connection, server or client: protocol events become the
    /// private ABI's event view and go straight to the generated C handler; the checkpoint
    /// drains the queues.
    struct NativeHttpDispatch {
        realm: std::rc::Weak<NativeRealm>,
        resource: kiln_host_abi::ResourceId,
        owner: Rc<HttpEventOwner>,
    }

    impl kiln_runtime::HttpDispatch for NativeHttpDispatch {
        fn event(&self, event: &kiln_runtime::HttpEvent<'_>) -> i32 {
            use kiln_runtime::HttpEventKind;
            let Some(realm) = self.realm.upgrade() else {
                return INVALID_ARGUMENT;
            };
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
            let status =
                unsafe { (self.owner.run)(self.owner.pointer, &raw const view, host, thrown) };
            if matches!(
                event.kind,
                HttpEventKind::Finish
                    | HttpEventKind::ResponseEnd
                    | HttpEventKind::Closed
                    | HttpEventKind::Error
            ) {
                // The drive may be waiting for exactly this request to finish.
                let runtime = realm.runtime.borrow();
                if !runtime.has_pending_host_work() {
                    runtime.wake_drive();
                }
            }
            if event.kind == HttpEventKind::Closed {
                realm.http_connections.borrow_mut().remove(&self.resource);
                let _ = realm
                    .runtime
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
            drain_realm_queues(&realm)
        }
    }

    fn attach_http(
        resource: u64,
        role: HttpRole,
        allow_half_close: bool,
        run: Option<RunHttpEvent>,
        drop: Option<DropWork>,
        work: *mut c_void,
    ) -> i32 {
        ffi(|| {
            let (Some(realm), Some(runtime), Some(run), Some(drop)) =
                (active_realm(), active_runtime(), run, drop)
            else {
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
            });
            // SAFETY: active_runtime points at this active realm's exclusively-entered runtime.
            let runtime = unsafe { &mut *runtime };
            let attached = match role {
                HttpRole::Server => runtime.server_attach(resource, allow_half_close, dispatch),
                HttpRole::Client => runtime.client_attach(resource, dispatch),
            };
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

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_server_attach_active(
        resource: u64,
        run: Option<RunHttpEvent>,
        drop: Option<DropWork>,
        work: *mut c_void,
    ) -> i32 {
        attach_http(resource, HttpRole::Server, false, run, drop, work)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_server_attach_with_options_active(
        resource: u64,
        allow_half_close: i32,
        run: Option<RunHttpEvent>,
        drop: Option<DropWork>,
        work: *mut c_void,
    ) -> i32 {
        attach_http(
            resource,
            HttpRole::Server,
            allow_half_close != 0,
            run,
            drop,
            work,
        )
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_client_attach_active(
        resource: u64,
        run: Option<RunHttpEvent>,
        drop: Option<DropWork>,
        work: *mut c_void,
    ) -> i32 {
        attach_http(resource, HttpRole::Client, false, run, drop, work)
    }

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

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_agent_acquire_active(
        host: *const u8,
        host_len: u64,
        port: u16,
        request: u64,
        out_resource: *mut u64,
    ) -> i32 {
        ffi(|| {
            let (Some(_realm), Some(runtime), Some(out_resource)) =
                (active_realm(), active_runtime(), unsafe {
                    out_resource.as_mut()
                })
            else {
                return INVALID_ARGUMENT;
            };
            let Some(host) = (unsafe { borrowed_bytes(host, host_len) }) else {
                return INVALID_ARGUMENT;
            };
            let Ok(host) = std::str::from_utf8(host) else {
                return INVALID_ARGUMENT;
            };
            *out_resource = 0;
            match unsafe { &mut *runtime }
                .acquire_client_connection(HttpOrigin::http(host, port), request)
            {
                AgentDecision::Connect => HTTP_AGENT_CONNECT,
                AgentDecision::Reuse(resource) => {
                    *out_resource = resource.get();
                    HTTP_AGENT_REUSE
                }
                AgentDecision::Queued => HTTP_AGENT_QUEUED,
            }
        })
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_client_owner_active(
        resource: u64,
        out_owner: *mut *mut c_void,
    ) -> i32 {
        ffi(|| {
            let (Some(realm), Some(out_owner)) = (active_realm(), unsafe { out_owner.as_mut() })
            else {
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

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_agent_connected_active(
        host: *const u8,
        host_len: u64,
        port: u16,
        resource: u64,
        request: u64,
    ) -> i32 {
        ffi(|| {
            let (Some(_realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            let Some(host) = (unsafe { borrowed_bytes(host, host_len) }) else {
                return INVALID_ARGUMENT;
            };
            let Ok(host) = std::str::from_utf8(host) else {
                return INVALID_ARGUMENT;
            };
            i32::from(unsafe { &mut *runtime }.client_connection_connected(
                HttpOrigin::http(host, port),
                ResourceId::new(resource),
                request,
            ))
        })
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_agent_release_active(
        resource: u64,
        reusable: i32,
        out_request: *mut u64,
    ) -> i32 {
        ffi(|| {
            let (Some(_realm), Some(runtime), Some(out_request)) =
                (active_realm(), active_runtime(), unsafe {
                    out_request.as_mut()
                })
            else {
                return INVALID_ARGUMENT;
            };
            *out_request = 0;
            match unsafe { &mut *runtime }
                .release_client_connection(ResourceId::new(resource), reusable != 0)
            {
                AgentRelease::Idle => HTTP_AGENT_IDLE,
                AgentRelease::Dispatch { request } => {
                    *out_request = request;
                    HTTP_AGENT_DISPATCH
                }
                AgentRelease::Retire => HTTP_AGENT_RETIRE,
            }
        })
    }

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
            let (Some(realm), Some(runtime)) = (active_realm(), active_runtime()) else {
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
            let Some(method) = (unsafe { borrowed_bytes(request.method, request.method_len) })
            else {
                return INVALID_ARGUMENT;
            };
            let Some(target) = (unsafe { borrowed_bytes(request.target, request.target_len) })
            else {
                return INVALID_ARGUMENT;
            };
            let Some(headers) = (unsafe { parse_headers(request.headers, request.headers_len) })
            else {
                return INVALID_ARGUMENT;
            };
            let (Ok(method), Ok(target)) =
                (std::str::from_utf8(method), std::str::from_utf8(target))
            else {
                return INVALID_ARGUMENT;
            };
            let Ok(high_water_mark) = usize::try_from(high_water_mark) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: active_runtime is this active realm's exclusively-entered runtime.
            let sent = unsafe { &mut *runtime }.http_client().send_request(
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

    fn write_http_client_request(
        resource: u64,
        id: u64,
        data: *const u8,
        data_len: u64,
        end: bool,
    ) -> i32 {
        ffi(|| {
            let (Some(_realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: the generated adapter keeps this span live until this call copies it.
            let Some(data) = (unsafe { borrowed_bytes(data, data_len) }) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: active_runtime is exclusively owned by this active realm callback.
            let client = unsafe { &mut *runtime }.http_client();
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

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_client_write_active(
        resource: u64,
        id: u64,
        data: *const u8,
        data_len: u64,
    ) -> i32 {
        write_http_client_request(resource, id, data, data_len, false)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_client_end_active(
        resource: u64,
        id: u64,
        data: *const u8,
        data_len: u64,
    ) -> i32 {
        write_http_client_request(resource, id, data, data_len, true)
    }

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
            let (Some(_realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: this private frame remains live for the synchronous call.
            let response = unsafe { &*response };
            let Ok(high_water_mark) = usize::try_from(high_water_mark) else {
                return INVALID_ARGUMENT;
            };
            let resource = kiln_host_abi::ResourceId::new(resource);
            // SAFETY: active_runtime is exclusively owned by the active realm callback.
            let runtime = unsafe { &mut *runtime };
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

    fn write_http_response(
        resource: u64,
        id: u64,
        data: *const u8,
        data_len: u64,
        end: bool,
    ) -> i32 {
        ffi(|| {
            let (Some(_realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: the adapter keeps this borrowed span live until the command copies it.
            let Some(data) = (unsafe { borrowed_bytes(data, data_len) }) else {
                return INVALID_ARGUMENT;
            };
            let resource = kiln_host_abi::ResourceId::new(resource);
            // SAFETY: active_runtime is exclusively owned by this active realm callback.
            let runtime = unsafe { &mut *runtime };
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

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_response_write_active(
        resource: u64,
        id: u64,
        data: *const u8,
        data_len: u64,
    ) -> i32 {
        write_http_response(resource, id, data, data_len, false)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_response_end_active(
        resource: u64,
        id: u64,
        data: *const u8,
        data_len: u64,
    ) -> i32 {
        write_http_response(resource, id, data, data_len, true)
    }

    fn set_timer(
        repeating: bool,
        delay_ms: f64,
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
            let Some(work) = NativeWork::new(work, run, drop) else {
                return INVALID_ARGUMENT;
            };
            let delay = if delay_ms.is_finite() && delay_ms > 0.0 {
                duration_from_milliseconds(delay_ms)
            } else {
                std::time::Duration::ZERO
            };
            let id = if repeating {
                queue.set_interval(delay, work)
            } else {
                queue.set_timeout(delay, work)
            };
            std::mem::drop(queue);
            // A timer armed from inside a connection's callback (ADR 0046) changes what the
            // parked drive must wait for; tell it.
            if let Some(runtime) = active_runtime() {
                // SAFETY: the active runtime pointer is valid for the whole drive.
                unsafe { &*runtime }.wake_drive();
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
        set_timer(false, delay_ms, run, drop, work, out_id)
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
        set_timer(true, delay_ms, run, drop, work, out_id)
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
            REALMS.with(|realms| {
                if realms.borrow_mut().remove(&handle).is_some() {
                    0
                } else {
                    INVALID_ARGUMENT
                }
            })
        })
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod tests {
        use std::error::Error;
        use std::sync::PoisonError;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        static CALLABLE_STORE_DROPS: AtomicUsize = AtomicUsize::new(0);

        fn opaque_test_pointer(identity: usize) -> *mut c_void {
            ptr::without_provenance_mut(identity)
        }

        unsafe extern "C" fn record_callable_store_drop(_pointer: *mut c_void) {
            CALLABLE_STORE_DROPS.fetch_add(1, AtomicOrdering::SeqCst);
        }

        use super::*;
        use std::fs;
        use std::ptr;
        use std::sync::atomic::{AtomicU64, Ordering};

        static TEST_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

        unsafe extern "C" fn exercises_realm_value_slots(context: *mut c_void) -> i32 {
            let slots = unsafe { &mut *context.cast::<[*mut c_void; 3]>() };
            if unsafe { kiln_realm_value_slot(1, &raw mut slots[0]) } != 0
                || unsafe { kiln_realm_value_slot(1, &raw mut slots[1]) } != 0
                || unsafe { kiln_realm_value_slot(2, &raw mut slots[2]) } != 0
            {
                return RUNTIME_ERROR;
            }
            unsafe { slots[0].cast::<u64>().write(37) };
            i32::from(unsafe { slots[1].cast::<u64>().read() } != 37)
        }

        #[test]
        fn opaque_value_slots_are_stable_distinct_and_realm_owned() {
            let mut first_realm = 0;
            let mut second_realm = 0;
            let mut first: [*mut c_void; 3] = [ptr::null_mut(); 3];
            let mut second: [*mut c_void; 3] = [ptr::null_mut(); 3];
            assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
            assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_realm,
                        Some(exercises_realm_value_slots),
                        (&raw mut first).cast(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        second_realm,
                        Some(exercises_realm_value_slots),
                        (&raw mut second).cast(),
                    )
                },
                0
            );
            assert_eq!(first[0], first[1]);
            assert_ne!(first[0], first[2]);
            assert_ne!(first[0], second[0]);
            assert_eq!(
                unsafe { kiln_realm_value_slot(1, ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            assert_eq!(kiln_realm_destroy(first_realm), 0);
            assert_eq!(kiln_realm_destroy(second_realm), 0);
        }

        unsafe extern "C" fn reads_realm_argv(context: *mut c_void) -> i32 {
            let observed = unsafe { &mut *context.cast::<Vec<Vec<u8>>>() };
            let mut count = 0_u64;
            if unsafe { kiln_realm_argv_count(&raw mut count) } != 0 {
                return RUNTIME_ERROR;
            }
            for index in 0..count {
                let mut bytes = ptr::null();
                let mut len = 0_u64;
                if unsafe { kiln_realm_argv_at(index, &raw mut bytes, &raw mut len) } != 0 {
                    return RUNTIME_ERROR;
                }
                observed.push(unsafe { std::slice::from_raw_parts(bytes, len as usize) }.to_vec());
            }
            // One past the end is a question with no answer, not an empty one.
            let mut bytes = ptr::null();
            let mut len = 0_u64;
            i32::from(
                unsafe { kiln_realm_argv_at(count, &raw mut bytes, &raw mut len) }
                    != INVALID_ARGUMENT,
            )
        }

        /// The launcher's `argv` is borrowed C memory owned by the OS. Copying it is what lets a
        /// program keep reading `process.argv` after `main`'s frame has moved on, and what lets
        /// this test hand over a buffer it then drops before the realm is ever entered.
        #[test]
        fn realm_argv_is_copied_from_the_launcher_and_read_back_in_order() {
            let mut realm = 0;
            assert_eq!(unsafe { kiln_realm_create(&raw mut realm) }, 0);
            {
                let owned: Vec<Vec<u8>> = [b"/tmp/program".as_slice(), b"--flag", b""]
                    .into_iter()
                    .map(|entry| {
                        let mut terminated = entry.to_vec();
                        terminated.push(0);
                        terminated
                    })
                    .collect();
                let pointers: Vec<*const c_char> = owned
                    .iter()
                    .map(|entry| entry.as_ptr().cast::<c_char>())
                    .collect();
                assert_eq!(
                    unsafe { kiln_realm_set_argv(realm, 3, pointers.as_ptr()) },
                    0
                );
            }
            let mut observed: Vec<Vec<u8>> = Vec::new();
            assert_eq!(
                unsafe {
                    kiln_realm_enter(realm, Some(reads_realm_argv), (&raw mut observed).cast())
                },
                0
            );
            assert_eq!(
                observed,
                vec![b"/tmp/program".to_vec(), b"--flag".to_vec(), Vec::new()]
            );
            // A realm nobody gave a command line reports none rather than guessing at one.
            let mut bare = 0;
            let mut none: Vec<Vec<u8>> = Vec::new();
            assert_eq!(unsafe { kiln_realm_create(&raw mut bare) }, 0);
            assert_eq!(
                unsafe { kiln_realm_enter(bare, Some(reads_realm_argv), (&raw mut none).cast()) },
                0
            );
            assert!(none.is_empty());
            assert_eq!(
                unsafe { kiln_realm_set_argv(0, 0, ptr::null()) },
                INVALID_ARGUMENT
            );
            assert_eq!(kiln_realm_destroy(realm), 0);
            assert_eq!(kiln_realm_destroy(bare), 0);
        }

        unsafe extern "C" fn draws_realm_entropy(context: *mut c_void) -> i32 {
            let words = unsafe { &mut *context.cast::<[u64; 2]>() };
            if unsafe { kiln_realm_entropy_word(7, &raw mut words[0]) } != 0
                || unsafe { kiln_realm_entropy_word(7, &raw mut words[1]) } != 0
            {
                return RUNTIME_ERROR;
            }
            let mut rejected = 0;
            i32::from(
                unsafe { kiln_realm_entropy_word(8, &raw mut rejected) } != INVALID_ARGUMENT
                    || rejected != 0,
            )
        }

        #[test]
        fn deterministic_entropy_is_one_realm_stream_and_rejects_policy_drift() {
            let mut first_realm = 0;
            let mut second_realm = 0;
            let mut first: [u64; 2] = [0; 2];
            let mut second: [u64; 2] = [0; 2];
            assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
            assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_realm,
                        Some(draws_realm_entropy),
                        (&raw mut first).cast(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        second_realm,
                        Some(draws_realm_entropy),
                        (&raw mut second).cast(),
                    )
                },
                0
            );
            assert_ne!(first[0], first[1]);
            assert_eq!(first, second);
            assert_eq!(
                unsafe { kiln_realm_entropy_word(7, ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            assert_eq!(kiln_realm_destroy(first_realm), 0);
            assert_eq!(kiln_realm_destroy(second_realm), 0);
        }

        unsafe extern "C" fn records_and_renders_diagnostic(context: *mut c_void) -> i32 {
            let site = b"node_modules/provider/index.js";
            let operation = b"CallMethod(listen)";
            let detail = b"unsupported receiver";
            if kiln_realm_diagnostic_reset() != 0
                || unsafe {
                    kiln_realm_diagnostic_note(
                        site.as_ptr(),
                        site.len() as u64,
                        37,
                        operation.as_ptr(),
                        operation.len() as u64,
                        3,
                        detail.as_ptr(),
                        detail.len() as u64,
                        ptr::null(),
                        0,
                        0,
                        19,
                        0,
                    )
                } != 0
            {
                return RUNTIME_ERROR;
            }
            let mut needed = 0;
            if unsafe { kiln_realm_diagnostic_render(ptr::null_mut(), 0, &raw mut needed) }
                != INVALID_ARGUMENT
                || needed == 0
            {
                return RUNTIME_ERROR;
            }
            let buffer = unsafe { &mut *context.cast::<Vec<u8>>() };
            buffer.resize(needed as usize, 0);
            unsafe { kiln_realm_diagnostic_render(buffer.as_mut_ptr(), needed, &raw mut needed) }
        }

        #[test]
        fn diagnostics_are_owned_and_rendered_by_the_active_realm() {
            let mut handle = 0;
            let mut rendered = Vec::new();
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(records_and_renders_diagnostic),
                        (&raw mut rendered).cast(),
                    )
                },
                0
            );
            assert_eq!(
                String::from_utf8(rendered),
                Ok("module=node_modules/provider/index.js instruction=37 operation=CallMethod(listen) detail=unsupported receiver receiver_tag=19".to_owned())
            );
            assert_eq!(kiln_realm_diagnostic_reset(), INVALID_ARGUMENT);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        unsafe extern "C" fn close_unregistered_http_connection(context: *mut c_void) -> i32 {
            // A missing HTTP registration is a failed capability lookup, never permission to
            // reinterpret this protocol socket as a raw node:net resource.
            unsafe {
                *context.cast::<i32>() = kiln_realm_http_server_close_active(42, HTTP_CLOSE_ABORT)
            };
            0
        }

        #[test]
        fn missing_http_registration_never_falls_back_to_raw_close() {
            let mut handle = 0;
            let mut observed = 0;
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(close_unregistered_http_connection),
                        (&raw mut observed).cast(),
                    )
                },
                0
            );
            assert_eq!(observed, INVALID_ARGUMENT);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        unsafe extern "C" fn exercises_symbol_registry(context: *mut c_void) -> i32 {
            let output = unsafe { &mut *context.cast::<[u64; 3]>() };
            let key = b"fastify-error-generic";
            let mut first = 0;
            let mut second = 0;
            let mut borrowed = std::ptr::null();
            let mut borrowed_len = 0;
            if unsafe { kiln_realm_symbol_for(key.as_ptr(), key.len() as u64, &raw mut first) } != 0
                || unsafe { kiln_realm_symbol_for(key.as_ptr(), key.len() as u64, &raw mut second) }
                    != 0
                || first != second
                || unsafe {
                    kiln_realm_symbol_key_for(first, &raw mut borrowed, &raw mut borrowed_len)
                } != 0
            {
                return RUNTIME_ERROR;
            }
            output[0] = first;
            output[1] = second;
            output[2] = borrowed_len;
            0
        }

        #[test]
        fn registered_symbols_are_realm_owned_and_stable() {
            let mut first_realm = 0;
            let mut second_realm = 0;
            let mut first = [0; 3];
            let mut second = [0; 3];
            assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
            assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_realm,
                        Some(exercises_symbol_registry),
                        (&raw mut first).cast(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        second_realm,
                        Some(exercises_symbol_registry),
                        (&raw mut second).cast(),
                    )
                },
                0
            );
            assert_eq!(first[0], first[1]);
            assert_eq!(second[0], second[1]);
            assert_ne!(first[0], second[0]);
            assert_eq!(first[2], b"fastify-error-generic".len() as u64);
            assert_eq!(second[2], b"fastify-error-generic".len() as u64);
            assert_eq!(kiln_realm_destroy(first_realm), 0);
            assert_eq!(kiln_realm_destroy(second_realm), 0);
        }

        unsafe extern "C" fn exercises_callable_store_registry(context: *mut c_void) -> i32 {
            let output = unsafe { &mut *context.cast::<[*mut c_void; 3]>() };
            let first_candidate = opaque_test_pointer(1);
            let duplicate_candidate = opaque_test_pointer(2);
            let other_candidate = opaque_test_pointer(3);
            if unsafe {
                kiln_realm_callable_store(8, 27, ptr::null_mut(), None, &raw mut output[0])
            } != 0
                || !output[0].is_null()
                || unsafe {
                    kiln_realm_callable_store(
                        8,
                        27,
                        first_candidate,
                        Some(record_callable_store_drop),
                        &raw mut output[0],
                    )
                } != 0
                || unsafe {
                    kiln_realm_callable_store(
                        8,
                        27,
                        duplicate_candidate,
                        Some(record_callable_store_drop),
                        &raw mut output[1],
                    )
                } != 0
                || unsafe {
                    kiln_realm_callable_store(
                        11,
                        27,
                        other_candidate,
                        Some(record_callable_store_drop),
                        &raw mut output[2],
                    )
                } != 0
            {
                return RUNTIME_ERROR;
            }
            0
        }

        unsafe extern "C" fn publishes_second_realm_callable_store(context: *mut c_void) -> i32 {
            let output = unsafe { &mut *context.cast::<*mut c_void>() };
            unsafe {
                kiln_realm_callable_store(
                    8,
                    27,
                    opaque_test_pointer(4),
                    Some(record_callable_store_drop),
                    output,
                )
            }
        }

        #[test]
        fn runtime_callable_stores_are_stable_distinct_and_realm_owned() {
            CALLABLE_STORE_DROPS.store(0, AtomicOrdering::SeqCst);
            let mut first_realm = 0;
            let mut second_realm = 0;
            let mut first = [ptr::null_mut(); 3];
            let mut second: *mut c_void = ptr::null_mut();
            assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
            assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_realm,
                        Some(exercises_callable_store_registry),
                        (&raw mut first).cast(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        second_realm,
                        Some(publishes_second_realm_callable_store),
                        (&raw mut second).cast(),
                    )
                },
                0
            );
            assert_eq!(first[0], opaque_test_pointer(1));
            assert_eq!(first[1], first[0]);
            assert_eq!(first[2], opaque_test_pointer(3));
            assert_eq!(second, opaque_test_pointer(4));
            assert_eq!(
                unsafe { kiln_realm_callable_store(8, 27, ptr::null_mut(), None, ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            assert_eq!(kiln_realm_destroy(first_realm), 0);
            assert_eq!(kiln_realm_destroy(second_realm), 0);
            assert_eq!(CALLABLE_STORE_DROPS.load(AtomicOrdering::SeqCst), 3);
        }

        #[test]
        fn the_collector_seam_marks_registered_roots_and_tears_down_once() {
            use std::sync::atomic::AtomicUsize;
            static COLLECTS: AtomicUsize = AtomicUsize::new(0);
            static BEGINS: AtomicUsize = AtomicUsize::new(0);
            static ENDS: AtomicUsize = AtomicUsize::new(0);
            static MARKED_VALUES: AtomicUsize = AtomicUsize::new(0);
            static ROOT_PHASES: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

            unsafe extern "C" fn collect(_heap: *mut c_void, _force: i32) {
                COLLECTS.fetch_add(1, AtomicOrdering::SeqCst);
            }
            unsafe extern "C" fn teardown_begin(_heap: *mut c_void) {
                BEGINS.fetch_add(1, AtomicOrdering::SeqCst);
            }
            unsafe extern "C" fn teardown_end(heap: *mut c_void) {
                ENDS.fetch_add(1, AtomicOrdering::SeqCst);
                // SAFETY: the test allocated this heap as one Box below.
                drop(unsafe { Box::from_raw(heap.cast::<[*const GcHooks; 2]>()) });
            }
            unsafe extern "C" fn mark_value(_heap: *mut c_void, _value: *const c_void) {
                MARKED_VALUES.fetch_add(1, AtomicOrdering::SeqCst);
            }
            unsafe extern "C" fn mark_block(_heap: *mut c_void, _pointer: *mut c_void) {}
            unsafe extern "C" fn introspect(_heap: *mut c_void) -> i32 {
                0
            }
            unsafe extern "C" fn roots(heap: *mut c_void, context: *mut c_void, phase: i32) {
                ROOT_PHASES
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(phase);
                if phase == GC_PHASE_MARK || phase == GC_PHASE_TEARDOWN {
                    assert!(!heap.is_null());
                    // SAFETY: the heap is live in both marking phases.
                    assert_eq!(unsafe { kiln_realm_gc_mark_value(heap, context) }, 0);
                }
            }
            static HOOKS: GcHooks = GcHooks {
                version: GC_HOOKS_VERSION,
                size: std::mem::size_of::<GcHooks>() as u32,
                collect: Some(collect),
                teardown_begin: Some(teardown_begin),
                teardown_end: Some(teardown_end),
                mark_value: Some(mark_value),
                mark_block: Some(mark_block),
                introspect: Some(introspect),
            };

            unsafe extern "C" fn enter(context: *mut c_void) -> i32 {
                let mut heap = std::ptr::null_mut();
                // SAFETY: every pointer handed over below stays live for the call.
                unsafe {
                    // No heap yet: attach reports null without an error.
                    assert_eq!(
                        kiln_realm_gc_attach(&raw const HOOKS, std::ptr::null_mut(), &raw mut heap),
                        0
                    );
                    assert!(heap.is_null());
                    let candidate = Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()]))
                        .cast::<c_void>();
                    assert_eq!(
                        kiln_realm_gc_attach(&raw const HOOKS, candidate, &raw mut heap),
                        0
                    );
                    assert_eq!(heap, candidate);
                    // A second candidate is refused in favour of the existing heap.
                    let other = Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()]))
                        .cast::<c_void>();
                    assert_eq!(
                        kiln_realm_gc_attach(&raw const HOOKS, other, &raw mut heap),
                        0
                    );
                    assert_eq!(heap, candidate);
                    drop(Box::from_raw(other.cast::<[*const GcHooks; 2]>()));
                    assert_eq!(
                        kiln_realm_gc_register_roots(
                            Some(roots),
                            context,
                            GC_ROOT_SURVIVES_TEARDOWN
                        ),
                        0
                    );
                    assert_eq!(
                        kiln_realm_gc_register_roots(
                            Some(roots),
                            context,
                            GC_ROOT_SURVIVES_TEARDOWN
                        ),
                        0
                    );
                    assert_eq!(kiln_realm_gc_mark_roots(heap), 0);
                }
                0
            }

            let mut handle = 0;
            let mut slot = [0u64; 2];
            // SAFETY: the output slot and the root context stay live through the calls.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe { kiln_realm_enter(handle, Some(enter), (&raw mut slot).cast::<c_void>()) },
                0
            );
            assert_eq!(
                ROOT_PHASES
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_slice(),
                &[GC_PHASE_MARK]
            );
            assert_eq!(MARKED_VALUES.load(AtomicOrdering::SeqCst), 1);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(BEGINS.load(AtomicOrdering::SeqCst), 1);
            assert_eq!(ENDS.load(AtomicOrdering::SeqCst), 1);
            assert_eq!(
                ROOT_PHASES
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_slice(),
                &[GC_PHASE_MARK, GC_PHASE_TEARDOWN, GC_PHASE_RESET]
            );
            assert_eq!(MARKED_VALUES.load(AtomicOrdering::SeqCst), 2);
            assert_eq!(COLLECTS.load(AtomicOrdering::SeqCst), 0);
        }

        /// The dump has to be offered where the collector sweeps, and the numbers it prints have
        /// to be readable from there: the prelude calls back into this archive in the middle of
        /// the hook, so anything the drive still had borrowed would panic instead of printing.
        #[test]
        fn the_turn_boundary_offers_the_dump_and_the_report_reads_the_runtime()
        -> Result<(), Box<dyn Error>> {
            use std::sync::atomic::AtomicUsize;
            static OFFERS: AtomicUsize = AtomicUsize::new(0);
            static REPORT: std::sync::Mutex<Option<(i32, u64, [u64; 9])>> =
                std::sync::Mutex::new(None);

            unsafe extern "C" fn collect(_heap: *mut c_void, _force: i32) {}
            unsafe extern "C" fn teardown_begin(_heap: *mut c_void) {}
            unsafe extern "C" fn teardown_end(heap: *mut c_void) {
                // SAFETY: the test allocated this heap as one Box below.
                drop(unsafe { Box::from_raw(heap.cast::<[*const GcHooks; 2]>()) });
            }
            unsafe extern "C" fn mark_value(_heap: *mut c_void, _value: *const c_void) {}
            unsafe extern "C" fn mark_block(_heap: *mut c_void, _pointer: *mut c_void) {}
            unsafe extern "C" fn introspect(_heap: *mut c_void) -> i32 {
                OFFERS.fetch_add(1, AtomicOrdering::SeqCst);
                let mut fields = [u64::MAX; 9];
                let mut count = u64::MAX;
                // SAFETY: both output spans are this frame's, and the hook runs at the turn
                // boundary with the realm active.
                let status =
                    unsafe { kiln_realm_runtime_report(fields.as_mut_ptr(), 9, &raw mut count) };
                *REPORT.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((status, count, fields));
                INTROSPECT_ARMED
            }
            static HOOKS: GcHooks = GcHooks {
                version: GC_HOOKS_VERSION,
                size: std::mem::size_of::<GcHooks>() as u32,
                collect: Some(collect),
                teardown_begin: Some(teardown_begin),
                teardown_end: Some(teardown_end),
                mark_value: Some(mark_value),
                mark_block: Some(mark_block),
                introspect: Some(introspect),
            };

            unsafe extern "C" fn enter(_context: *mut c_void) -> i32 {
                let mut heap = std::ptr::null_mut();
                let candidate =
                    Box::into_raw(Box::new([&raw const HOOKS, std::ptr::null()])).cast::<c_void>();
                // SAFETY: the candidate stays live until teardown frees it.
                unsafe { kiln_realm_gc_attach(&raw const HOOKS, candidate, &raw mut heap) }
            }

            let mut handle = 0;
            // SAFETY: the output slot stays live through the call.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe { kiln_realm_enter(handle, Some(enter), std::ptr::null_mut()) },
                0
            );
            assert_eq!(
                unsafe { kiln_realm_drive(handle, std::ptr::null_mut(), std::ptr::null_mut()) },
                0
            );
            assert_eq!(OFFERS.load(AtomicOrdering::SeqCst), 1);

            let (status, count, fields) = REPORT
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .ok_or("the hook reported")?;
            assert_eq!(status, 0);
            assert_eq!(count, RUNTIME_REPORT_FIELDS as u64);
            // A realm runtime owns exactly one worker, and this one served nothing.
            assert_eq!(fields[5], 1, "workers");
            assert_eq!(
                &fields[6..9],
                &[0, 0, 0],
                "connections and requests in flight"
            );
            assert_eq!(kiln_realm_destroy(handle), 0);
            Ok(())
        }

        /// The park cap exists so an idle server answers a signal; it must not touch a
        /// virtual-clock realm, whose park is free by construction and whose determinism would
        /// otherwise start depending on real time.
        #[test]
        fn a_park_is_capped_only_for_an_armed_wall_clock_realm() -> Result<(), Box<dyn Error>> {
            let realm = NativeRealm::new()?;
            let brief = std::time::Duration::from_millis(5);
            let hour = std::time::Duration::from_secs(3600);

            assert_eq!(introspect_park(&realm, None), None);
            realm.introspect_armed.set(true);
            assert_eq!(introspect_park(&realm, None), None, "virtual clock");
            assert_eq!(
                introspect_park(&realm, Some(hour)),
                Some(hour),
                "virtual clock"
            );

            realm.clock.set(NativeClock::Wall {
                epoch: std::time::Duration::ZERO,
                started: std::time::Instant::now(),
            });
            assert_eq!(introspect_park(&realm, None), Some(INTROSPECT_POLL));
            assert_eq!(introspect_park(&realm, Some(hour)), Some(INTROSPECT_POLL));
            assert_eq!(introspect_park(&realm, Some(brief)), Some(brief));

            realm.introspect_armed.set(false);
            assert_eq!(introspect_park(&realm, None), None);
            assert_eq!(introspect_park(&realm, Some(hour)), Some(hour));
            Ok(())
        }

        struct RecordedWork {
            value: u8,
            order: *mut Vec<u8>,
            drops: *mut usize,
            clear: u64,
        }

        struct ReadObservation {
            status: i32,
            request: u64,
            sequence: u64,
            bytes: Vec<u8>,
            error_code: Vec<u8>,
            drops: usize,
        }

        struct ReadSubmission {
            handle: u64,
            path: Vec<u8>,
            observation: *mut ReadObservation,
            request: *mut u64,
        }

        struct ReadAndTimerSubmission {
            read: ReadSubmission,
            order: *mut Vec<u8>,
            drops: *mut usize,
        }

        #[derive(Default)]
        struct HostObservation {
            status: i32,
            request: u64,
            sequence: u64,
            value_kind: u32,
            bytes: Vec<u8>,
            value_u64: u64,
            resource: u64,
            resource_kind: u32,
            resource_port: u32,
            resource_flags: u32,
            address: Vec<u8>,
            peer_address: Vec<u8>,
            peer_port: u32,
            drops: usize,
        }

        struct HostWork {
            observation: *mut HostObservation,
        }

        unsafe extern "C" fn record_host_completion(
            work: *mut c_void,
            completion: *const CompletionView,
            _host: *mut c_void,
            _thrown: *mut c_void,
        ) -> i32 {
            if work.is_null() || completion.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: this continuation and completion view are owned by the current request.
            let target = unsafe { &mut *(*work.cast::<HostWork>()).observation };
            // SAFETY: the runtime keeps the completion view and all referenced bytes live until
            // this callback returns.
            let completion = unsafe { &*completion };
            target.status = completion.status;
            target.request = completion.request;
            target.sequence = completion.sequence;
            target.value_kind = completion.value_kind;
            target.value_u64 = completion.value_u64;
            target.resource = completion.resource;
            target.resource_kind = completion.resource_kind;
            target.resource_port = completion.resource_port;
            target.resource_flags = completion.resource_flags;
            target.peer_port = completion.peer_port;
            for (pointer, length, destination) in [
                (completion.bytes, completion.bytes_len, &mut target.bytes),
                (
                    completion.address,
                    completion.address_len,
                    &mut target.address,
                ),
                (
                    completion.peer_address,
                    completion.peer_address_len,
                    &mut target.peer_address,
                ),
            ] {
                let length = usize::try_from(length).unwrap_or(usize::MAX);
                if length != 0 && pointer.is_null() {
                    return INVALID_ARGUMENT;
                }
                // SAFETY: each completion byte span is borrowed for this callback.
                *destination = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
            }
            0
        }

        unsafe extern "C" fn drop_host_work(work: *mut c_void) {
            if work.is_null() {
                return;
            }
            // SAFETY: the realm invokes the matching disposer exactly once.
            let work = unsafe { Box::from_raw(work.cast::<HostWork>()) };
            // SAFETY: the test observation outlives the request and realm.
            unsafe {
                (*work.observation).drops = (*work.observation).drops.saturating_add(1);
            }
        }

        unsafe fn start_test_host_op(
            handle: u64,
            operation: &HostOpView,
            observation: *mut HostObservation,
            request: *mut u64,
        ) -> i32 {
            let work = Box::into_raw(Box::new(HostWork { observation })).cast::<c_void>();
            // SAFETY: every pointer remains live through submission and the continuation targets
            // remain live through realm drive.
            let status = unsafe {
                kiln_realm_start_host_op(
                    handle,
                    operation,
                    Some(record_host_completion),
                    Some(drop_host_work),
                    work,
                    request,
                )
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_host_work(work) };
            }
            status
        }

        unsafe fn start_test_active_host_op(
            operation: &HostOpView,
            observation: *mut HostObservation,
            request: *mut u64,
        ) -> i32 {
            let work = Box::into_raw(Box::new(HostWork { observation })).cast::<c_void>();
            // SAFETY: the active entry copies the frame before returning and owns the continuation
            // only when submission succeeds.
            let status = unsafe {
                kiln_realm_start_active_host_op(
                    operation,
                    Some(record_host_completion),
                    Some(drop_host_work),
                    work,
                    request,
                )
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_host_work(work) };
            }
            status
        }

        struct RegisteredHttpCloseAttempt {
            status: i32,
            request: u64,
            observation: HostObservation,
        }

        unsafe extern "C" fn ignore_http_event(
            _work: *mut c_void,
            _event: *const HttpEventView,
            _host: *mut c_void,
            _thrown: *mut c_void,
        ) -> i32 {
            0
        }

        unsafe extern "C" fn retain_test_http_owner(_work: *mut c_void) {}

        unsafe extern "C" fn try_raw_close_on_registered_http(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test keeps this state live until after realm destruction.
            let attempt = unsafe { &mut *context.cast::<RegisteredHttpCloseAttempt>() };
            let Some(realm) = active_realm() else {
                return INVALID_ARGUMENT;
            };
            let resource = ResourceId::new(42);
            realm.http_connections.borrow_mut().insert(
                resource,
                NativeHttpConnection {
                    role: HttpRole::Server,
                    events: Rc::new(HttpEventOwner {
                        pointer: context,
                        run: ignore_http_event,
                        drop: retain_test_http_owner,
                    }),
                },
            );
            let operation = HostOpView {
                operation: OP_CLOSE_RESOURCE,
                flags: 0,
                resource: resource.get(),
                argument: 0,
                text: ptr::null(),
                text_len: 0,
                bytes: ptr::null(),
                bytes_len: 0,
            };
            // SAFETY: the operation is copied synchronously and all observation slots stay live.
            attempt.status = unsafe {
                start_test_active_host_op(
                    &operation,
                    &raw mut attempt.observation,
                    &raw mut attempt.request,
                )
            };
            0
        }

        #[test]
        fn registered_http_connection_rejects_raw_hostops_close() {
            let mut handle = 0;
            let mut attempt = RegisteredHttpCloseAttempt {
                status: 0,
                request: 0,
                observation: HostObservation::default(),
            };
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(try_raw_close_on_registered_http),
                        (&raw mut attempt).cast(),
                    )
                },
                0
            );
            assert_eq!(attempt.status, INVALID_ARGUMENT);
            assert_eq!(attempt.request, 0);
            assert_eq!(attempt.observation.drops, 1);
            assert!(
                realm(handle)
                    .is_some_and(|registered| registered.continuations.borrow().is_empty()),
                "realm remains registered through the test, with no continuations left"
            );
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        unsafe extern "C" fn record_read_completion(
            work: *mut c_void,
            completion: *const CompletionView,
            _host: *mut c_void,
            _thrown: *mut c_void,
        ) -> i32 {
            if work.is_null() || completion.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the request owns this continuation and the completion view for the
            // callback's duration.
            let observation = unsafe { &mut *work.cast::<ReadWork>() }.observation;
            let completion = unsafe { &*completion };
            // SAFETY: completion pointers remain valid until this callback returns.
            let target = unsafe { &mut *observation };
            target.status = completion.status;
            target.request = completion.request;
            target.sequence = completion.sequence;
            if completion.status == 0 {
                let length = usize::try_from(completion.bytes_len).unwrap_or(usize::MAX);
                if length > 0 && completion.bytes.is_null() {
                    return INVALID_ARGUMENT;
                }
                // SAFETY: the runtime owns the bytes for this callback.
                target.bytes =
                    unsafe { std::slice::from_raw_parts(completion.bytes, length) }.to_vec();
            } else {
                let length = usize::try_from(completion.error_code_len).unwrap_or(usize::MAX);
                if length > 0 && completion.error_code.is_null() {
                    return INVALID_ARGUMENT;
                }
                // SAFETY: the runtime owns the error code for this callback.
                target.error_code =
                    unsafe { std::slice::from_raw_parts(completion.error_code, length) }.to_vec();
            }
            0
        }

        struct ReadWork {
            observation: *mut ReadObservation,
        }

        unsafe extern "C" fn drop_read_work(work: *mut c_void) {
            if work.is_null() {
                return;
            }
            // SAFETY: the continuation is dropped exactly once by the Realm owner.
            let work = unsafe { Box::from_raw(work.cast::<ReadWork>()) };
            // SAFETY: the observation outlives the request and its disposer.
            unsafe { (*work.observation).drops = (*work.observation).drops.saturating_add(1) };
        }

        unsafe extern "C" fn submit_read(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test owns the submission for the duration of realm entry.
            let submission = unsafe { &mut *context.cast::<ReadSubmission>() };
            let work = Box::new(ReadWork {
                observation: submission.observation,
            });
            let pointer = Box::into_raw(work).cast::<c_void>();
            // SAFETY: all pointers refer to test-owned storage live through the Realm drive.
            unsafe {
                kiln_realm_read_file(
                    submission.handle,
                    submission.path.as_ptr(),
                    u64::try_from(submission.path.len()).unwrap_or(u64::MAX),
                    Some(record_read_completion),
                    Some(drop_read_work),
                    pointer,
                    submission.request,
                )
            }
        }

        unsafe extern "C" fn submit_and_cancel(context: *mut c_void) -> i32 {
            // SAFETY: the caller provides the same live submission used by `submit_read`.
            let status = unsafe { submit_read(context) };
            if status != 0 {
                return status;
            }
            // SAFETY: `context` remains live and contains the request just submitted.
            let submission = unsafe { &mut *context.cast::<ReadSubmission>() };
            unsafe { kiln_realm_cancel_request(submission.handle, *submission.request) }
        }

        unsafe extern "C" fn submit_read_and_timer(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test owns this combined submission for the duration of realm entry.
            let submission = unsafe { &mut *context.cast::<ReadAndTimerSubmission>() };
            // SAFETY: the nested read submission and every callback target remain live through
            // the subsequent realm drive.
            let status = unsafe { submit_read((&raw mut submission.read).cast::<c_void>()) };
            if status != 0 {
                return status;
            }
            // SAFETY: both target pointers are test-owned until realm teardown.
            let timer = unsafe { recorded_work(9, &mut *submission.order, &mut *submission.drops) };
            let mut id = 0;
            // SAFETY: callbacks match the transferred allocation and the identity slot is live.
            let status = unsafe {
                kiln_realm_set_timeout(
                    3_600_000.0,
                    Some(record_work),
                    Some(drop_recorded_work),
                    timer,
                    &raw mut id,
                )
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_recorded_work(timer) };
            }
            status
        }

        struct CancelSubmission {
            handle: u64,
            request: u64,
            result: *mut i32,
        }

        unsafe extern "C" fn cancel_request(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test owns this context for the duration of realm entry.
            let submission = unsafe { &mut *context.cast::<CancelSubmission>() };
            let result =
                unsafe { kiln_realm_cancel_request(submission.handle, submission.request) };
            // SAFETY: the result slot remains live for this callback.
            unsafe { submission.result.write(result) };
            0
        }

        unsafe extern "C" fn record_work(
            work: *mut c_void,
            _host: *mut c_void,
            _thrown: *mut c_void,
        ) -> i32 {
            if work.is_null() {
                return INVALID_ARGUMENT;
            }
            let work = work.cast::<RecordedWork>();
            // SAFETY: the scheduling test keeps the record and both target allocations live until
            // Realm teardown invokes `drop_recorded_work`.
            unsafe {
                (*(*work).order).push((*work).value);
                if (*work).value == 5 {
                    let nested = recorded_work(6, &mut *(*work).order, &mut *(*work).drops);
                    let status = kiln_realm_queue_next_tick(
                        Some(record_work),
                        Some(drop_recorded_work),
                        nested,
                    );
                    if status != 0 {
                        drop_recorded_work(nested);
                        return status;
                    }
                }
                if (*work).clear != 0 {
                    return kiln_realm_clear_timer((*work).clear);
                }
            }
            0
        }

        unsafe extern "C" fn fail_recorded_work(
            work: *mut c_void,
            _host: *mut c_void,
            _thrown: *mut c_void,
        ) -> i32 {
            if work.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test owns the record until Realm drive drops the popped payload.
            let work = work.cast::<RecordedWork>();
            unsafe {
                (*(*work).order).push((*work).value);
            }
            RUNTIME_ERROR
        }

        unsafe extern "C" fn drop_recorded_work(work: *mut c_void) {
            if work.is_null() {
                return;
            }
            // SAFETY: Realm ownership invokes this exactly once for the pointer transferred by
            // the scheduling test.
            let work = unsafe { Box::from_raw(work.cast::<RecordedWork>()) };
            // SAFETY: the test's drop counter outlives Realm teardown.
            unsafe { *work.drops = (*work.drops).saturating_add(1) };
        }

        fn recorded_work(value: u8, order: &mut Vec<u8>, drops: &mut usize) -> *mut c_void {
            Box::into_raw(Box::new(RecordedWork {
                value,
                order,
                drops,
                clear: 0,
            }))
            .cast::<c_void>()
        }

        unsafe extern "C" fn schedule_recorded_work(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            // SAFETY: the test passes live `(Vec<u8>, usize)` storage for this entry call.
            let (order, drops) = unsafe { &mut *context.cast::<(Vec<u8>, usize)>() };
            let next_tick = recorded_work(5, order, drops);
            // SAFETY: callbacks match the transferred allocation.
            let mut status = unsafe {
                kiln_realm_queue_next_tick(Some(record_work), Some(drop_recorded_work), next_tick)
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_recorded_work(next_tick) };
                return status;
            }
            let microtask = recorded_work(1, order, drops);
            // SAFETY: callbacks match the transferred allocation.
            status = unsafe {
                kiln_realm_queue_microtask(Some(record_work), Some(drop_recorded_work), microtask)
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_recorded_work(microtask) };
                return status;
            }

            for (value, delay) in [(2, 0.0), (4, 10.0)] {
                let timer = recorded_work(value, order, drops);
                let mut id = 0;
                // SAFETY: callbacks, allocation, and identity slot satisfy the timer contract.
                status = unsafe {
                    kiln_realm_set_timeout(
                        delay,
                        Some(record_work),
                        Some(drop_recorded_work),
                        timer,
                        &raw mut id,
                    )
                };
                if status != 0 {
                    // SAFETY: ownership did not transfer on failure.
                    unsafe { drop_recorded_work(timer) };
                    return status;
                }
            }

            let interval = recorded_work(3, order, drops);
            let mut interval_id = 0;
            // SAFETY: callbacks, allocation, and identity slot satisfy the interval contract.
            status = unsafe {
                kiln_realm_set_interval(
                    5.0,
                    Some(record_work),
                    Some(drop_recorded_work),
                    interval,
                    &raw mut interval_id,
                )
            };
            if status != 0 {
                // SAFETY: ownership did not transfer on failure.
                unsafe { drop_recorded_work(interval) };
                return status;
            }
            // SAFETY: the allocation remains Realm-owned and live until the interval is cleared.
            unsafe { (*interval.cast::<RecordedWork>()).clear = interval_id };
            0
        }

        unsafe extern "C" fn sees_tokio(context: *mut c_void) -> i32 {
            if context.is_null() || tokio::runtime::Handle::try_current().is_err() {
                return RUNTIME_ERROR;
            }
            // SAFETY: the test passes a live `bool` slot.
            unsafe { context.cast::<bool>().write(true) };
            17
        }

        unsafe extern "C" fn reads_realm_clock(context: *mut c_void) -> i32 {
            // SAFETY: the test supplies one live `f64` slot while this callback runs.
            unsafe { kiln_realm_now(context.cast::<f64>()) }
        }

        unsafe extern "C" fn allocates_http_request_id(context: *mut c_void) -> i32 {
            // SAFETY: the test supplies one live `u64` slot while this callback runs.
            unsafe { kiln_realm_http_client_request_id_active(context.cast::<u64>()) }
        }

        unsafe extern "C" fn exercises_http_agent(context: *mut c_void) -> i32 {
            if context.is_null() {
                return INVALID_ARGUMENT;
            }
            let host = b"Example.COM";
            let mut resource = 0;
            let acquired = unsafe {
                kiln_realm_http_agent_acquire_active(
                    host.as_ptr(),
                    host.len() as u64,
                    80,
                    1,
                    &raw mut resource,
                )
            };
            if acquired != HTTP_AGENT_CONNECT || resource != 0 {
                return RUNTIME_ERROR;
            }
            if unsafe {
                kiln_realm_http_agent_connected_active(host.as_ptr(), host.len() as u64, 80, 91, 1)
            } != 1
            {
                return RUNTIME_ERROR;
            }
            let mut request = 0;
            if unsafe { kiln_realm_http_agent_release_active(91, 1, &raw mut request) }
                != HTTP_AGENT_IDLE
            {
                return RUNTIME_ERROR;
            }
            let reused = unsafe {
                kiln_realm_http_agent_acquire_active(
                    host.as_ptr(),
                    host.len() as u64,
                    80,
                    2,
                    &raw mut resource,
                )
            };
            if reused != HTTP_AGENT_REUSE || resource != 91 {
                return RUNTIME_ERROR;
            }
            unsafe { context.cast::<u64>().write(resource) };
            0
        }

        #[test]
        fn opaque_handle_enters_tokio_and_rejects_stale_or_cross_thread_use() {
            let mut handle = 0;
            // SAFETY: `handle` is a live writable slot.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_ne!(handle, 0);
            let mut entered = false;
            // SAFETY: callback and context satisfy `sees_tokio`'s contract.
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(sees_tokio),
                        (&raw mut entered).cast::<c_void>(),
                    )
                },
                17
            );
            assert!(entered);

            let foreign = std::thread::spawn(move || {
                // SAFETY: null context is accepted by this callback only after handle lookup;
                // the foreign thread must reject the handle before invoking it.
                unsafe { kiln_realm_enter(handle, Some(sees_tokio), ptr::null_mut()) }
            })
            .join()
            .unwrap_or(RUNTIME_ERROR);
            assert_eq!(foreign, INVALID_ARGUMENT);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(kiln_realm_destroy(handle), INVALID_ARGUMENT);
        }

        #[test]
        fn http_client_request_ids_are_monotonic_and_realm_scoped() {
            let mut first_realm = 0;
            let mut second_realm = 0;
            assert_eq!(unsafe { kiln_realm_create(&raw mut first_realm) }, 0);
            assert_eq!(unsafe { kiln_realm_create(&raw mut second_realm) }, 0);

            let mut first = 0;
            let mut second = 0;
            let mut other_realm = 0;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_realm,
                        Some(allocates_http_request_id),
                        (&raw mut first).cast(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_realm,
                        Some(allocates_http_request_id),
                        (&raw mut second).cast(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        second_realm,
                        Some(allocates_http_request_id),
                        (&raw mut other_realm).cast(),
                    )
                },
                0
            );
            assert_eq!((first, second, other_realm), (1, 2, 1));
            assert_eq!(kiln_realm_destroy(first_realm), 0);
            assert_eq!(kiln_realm_destroy(second_realm), 0);
        }

        #[test]
        fn native_http_agent_abi_maps_runtime_decisions_without_rust_layout() {
            let mut handle = 0;
            let mut reused = 0;
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(handle, Some(exercises_http_agent), (&raw mut reused).cast())
                },
                0
            );
            assert_eq!(reused, 91);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        #[test]
        fn null_slots_and_callbacks_fail_closed() {
            // SAFETY: the null pointer is intentionally rejected without dereferencing.
            assert_eq!(
                unsafe { kiln_realm_create(ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            // SAFETY: the absent callback is intentionally rejected.
            assert_eq!(
                unsafe { kiln_realm_enter(1, None, ptr::null_mut()) },
                INVALID_ARGUMENT
            );
            // SAFETY: the null slot is intentionally rejected without dereferencing.
            assert_eq!(unsafe { kiln_realm_now(ptr::null_mut()) }, INVALID_ARGUMENT);
            // SAFETY: no Realm is active, so the opaque callback cannot be accepted.
            assert_eq!(
                unsafe { kiln_realm_queue_next_tick(None, None, ptr::null_mut()) },
                INVALID_ARGUMENT
            );
        }

        #[test]
        fn async_read_returns_identity_and_resumes_only_during_drive() {
            let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-native-async-{suffix}.txt"));
            assert!(fs::write(&path, b"native continuation").is_ok());
            let path_bytes = path.to_string_lossy().as_bytes().to_vec();
            let mut handle = 0;
            let mut request = 0;
            let mut observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut submission = ReadSubmission {
                handle: 0,
                path: path_bytes,
                observation: &raw mut observation,
                request: &raw mut request,
            };
            // SAFETY: all slots and callback state remain live until after drive and teardown.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            submission.handle = handle;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_read),
                        (&raw mut submission).cast::<c_void>(),
                    )
                },
                0
            );
            assert_ne!(request, 0);
            assert_eq!(observation.status, -1);
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(observation.status, 0);
            assert_eq!(observation.request, request);
            assert_eq!(observation.sequence, 0);
            assert_eq!(observation.bytes, b"native continuation");
            assert_eq!(observation.drops, 1);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert!(fs::remove_file(path).is_ok());
        }

        #[test]
        fn native_timer_is_not_starved_by_pending_async_io() {
            let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-native-turn-{suffix}.txt"));
            assert!(fs::write(&path, b"completion").is_ok());
            let mut handle = 0;
            let mut request = 0;
            let mut observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut order = Vec::new();
            let mut timer_drops = 0;
            let mut submission = ReadAndTimerSubmission {
                read: ReadSubmission {
                    handle: 0,
                    path: path.to_string_lossy().as_bytes().to_vec(),
                    observation: &raw mut observation,
                    request: &raw mut request,
                },
                order: &raw mut order,
                drops: &raw mut timer_drops,
            };
            // SAFETY: callback state remains live through drive and realm teardown.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            submission.read.handle = handle;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_read_and_timer),
                        (&raw mut submission).cast::<c_void>(),
                    )
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(order, [9]);
            assert_eq!(observation.status, 0);
            assert_eq!(observation.bytes, b"completion");
            assert_eq!(observation.drops, 1);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(timer_drops, 1);
            assert!(fs::remove_file(path).is_ok());
        }

        #[test]
        fn generic_native_host_ops_exchange_live_tcp_bytes_and_resources() {
            struct TcpExchange {
                handle: u64,
                stage: u8,
                listener: HostObservation,
                accepted: HostObservation,
                client: HostObservation,
                write: HostObservation,
                read: HostObservation,
                unref: HostObservation,
                state: HostObservation,
                close_listener: HostObservation,
                close_client: HostObservation,
                close_accepted: HostObservation,
                requests: [u64; 10],
            }

            unsafe extern "C" fn submit_stage(context: *mut c_void) -> i32 {
                if context.is_null() {
                    return INVALID_ARGUMENT;
                }
                // SAFETY: the test keeps this exchange live through every entry and drive.
                let exchange = unsafe { &mut *context.cast::<TcpExchange>() };
                let host = b"127.0.0.1";
                match exchange.stage {
                    0 => {
                        let operation = HostOpView {
                            operation: OP_TCP_LISTEN,
                            flags: 1,
                            resource: 0,
                            argument: 0,
                            text: host.as_ptr(),
                            text_len: host.len() as u64,
                            bytes: ptr::null(),
                            bytes_len: 0,
                        };
                        // SAFETY: operation framing and callback targets are live.
                        unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &operation,
                                &raw mut exchange.listener,
                                &raw mut exchange.requests[0],
                            )
                        }
                    }
                    1 => {
                        let accept = HostOpView {
                            operation: OP_TCP_ACCEPT,
                            flags: 1,
                            resource: exchange.listener.resource,
                            argument: 0,
                            text: ptr::null(),
                            text_len: 0,
                            bytes: ptr::null(),
                            bytes_len: 0,
                        };
                        let connect = HostOpView {
                            operation: OP_TCP_CONNECT,
                            flags: 1,
                            resource: 0,
                            argument: u64::from(exchange.listener.resource_port),
                            text: host.as_ptr(),
                            text_len: host.len() as u64,
                            bytes: ptr::null(),
                            bytes_len: 0,
                        };
                        // SAFETY: both operations copy their frames before returning.
                        let status = unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &accept,
                                &raw mut exchange.accepted,
                                &raw mut exchange.requests[1],
                            )
                        };
                        if status != 0 {
                            return status;
                        }
                        // SAFETY: callback targets remain live through the following drive.
                        unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &connect,
                                &raw mut exchange.client,
                                &raw mut exchange.requests[2],
                            )
                        }
                    }
                    2 => {
                        let bytes = b"native tcp";
                        let write = HostOpView {
                            operation: OP_TCP_WRITE,
                            flags: 1,
                            resource: exchange.client.resource,
                            argument: 0,
                            text: ptr::null(),
                            text_len: 0,
                            bytes: bytes.as_ptr(),
                            bytes_len: bytes.len() as u64,
                        };
                        let read = HostOpView {
                            operation: OP_TCP_READ,
                            flags: 1,
                            resource: exchange.accepted.resource,
                            argument: 64,
                            text: ptr::null(),
                            text_len: 0,
                            bytes: ptr::null(),
                            bytes_len: 0,
                        };
                        // SAFETY: the write bytes are copied and callback targets remain live.
                        let status = unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &read,
                                &raw mut exchange.read,
                                &raw mut exchange.requests[3],
                            )
                        };
                        if status != 0 {
                            return status;
                        }
                        // SAFETY: operation framing is live through submission.
                        unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &write,
                                &raw mut exchange.write,
                                &raw mut exchange.requests[4],
                            )
                        }
                    }
                    3 => {
                        let unref = HostOpView {
                            operation: OP_SET_RESOURCE_REFERENCED,
                            flags: 0,
                            resource: exchange.client.resource,
                            argument: 0,
                            text: ptr::null(),
                            text_len: 0,
                            bytes: ptr::null(),
                            bytes_len: 0,
                        };
                        // SAFETY: operation framing is live through submission.
                        unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &unref,
                                &raw mut exchange.unref,
                                &raw mut exchange.requests[5],
                            )
                        }
                    }
                    4 => {
                        let state = HostOpView {
                            operation: OP_RESOURCE_STATE,
                            flags: 0,
                            resource: exchange.client.resource,
                            argument: 0,
                            text: ptr::null(),
                            text_len: 0,
                            bytes: ptr::null(),
                            bytes_len: 0,
                        };
                        // SAFETY: operation framing is live through submission.
                        unsafe {
                            start_test_host_op(
                                exchange.handle,
                                &state,
                                &raw mut exchange.state,
                                &raw mut exchange.requests[6],
                            )
                        }
                    }
                    5 => {
                        let resources = [
                            (exchange.listener.resource, &raw mut exchange.close_listener),
                            (exchange.client.resource, &raw mut exchange.close_client),
                            (exchange.accepted.resource, &raw mut exchange.close_accepted),
                        ];
                        for (offset, (resource, observation)) in resources.into_iter().enumerate() {
                            let close = HostOpView {
                                operation: OP_CLOSE_RESOURCE,
                                flags: 0,
                                resource,
                                argument: 0,
                                text: ptr::null(),
                                text_len: 0,
                                bytes: ptr::null(),
                                bytes_len: 0,
                            };
                            // SAFETY: each operation is copied and each callback target remains
                            // live through the following drive.
                            let handle = exchange.handle;
                            let Some(request) = exchange.requests.get_mut(7 + offset) else {
                                return INVALID_ARGUMENT;
                            };
                            let status = unsafe {
                                start_test_host_op(handle, &close, observation, &raw mut *request)
                            };
                            if status != 0 {
                                return status;
                            }
                        }
                        0
                    }
                    _ => INVALID_ARGUMENT,
                }
            }

            let mut handle = 0;
            let mut exchange = TcpExchange {
                handle: 0,
                stage: 0,
                listener: HostObservation::default(),
                accepted: HostObservation::default(),
                client: HostObservation::default(),
                write: HostObservation::default(),
                read: HostObservation::default(),
                unref: HostObservation::default(),
                state: HostObservation::default(),
                close_listener: HostObservation::default(),
                close_client: HostObservation::default(),
                close_accepted: HostObservation::default(),
                requests: [0; 10],
            };
            // SAFETY: handle and exchange storage remain live through the complete test.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            exchange.handle = handle;
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            for stage in 0..=5 {
                exchange.stage = stage;
                // SAFETY: callback and context satisfy their contracts.
                assert_eq!(
                    unsafe {
                        kiln_realm_enter(
                            handle,
                            Some(submit_stage),
                            (&raw mut exchange).cast::<c_void>(),
                        )
                    },
                    0
                );
                // SAFETY: host and thrown slots remain live for every continuation in this drive.
                assert_eq!(
                    unsafe {
                        kiln_realm_drive(
                            handle,
                            (&raw mut host).cast::<c_void>(),
                            (&raw mut thrown).cast::<c_void>(),
                        )
                    },
                    0
                );
            }

            assert_eq!(exchange.listener.value_kind, VALUE_RESOURCE);
            assert_eq!(exchange.listener.resource_kind, RESOURCE_LISTENER);
            assert_eq!(exchange.listener.address, b"127.0.0.1");
            assert_ne!(exchange.listener.resource_port, 0);
            assert_eq!(exchange.accepted.value_kind, VALUE_ACCEPTED_STREAM);
            assert_eq!(exchange.accepted.resource_kind, RESOURCE_STREAM);
            assert_eq!(exchange.accepted.peer_address, b"127.0.0.1");
            assert_eq!(exchange.client.value_kind, VALUE_RESOURCE);
            assert_eq!(exchange.client.resource_kind, RESOURCE_STREAM);
            assert_eq!(exchange.write.value_kind, VALUE_UNDEFINED);
            assert_eq!(exchange.read.value_kind, VALUE_BYTES);
            assert_eq!(exchange.read.bytes, b"native tcp");
            assert_eq!(exchange.unref.value_kind, VALUE_UNDEFINED);
            assert_eq!(exchange.state.value_kind, VALUE_RESOURCE_STATE);
            assert_eq!(exchange.state.resource, exchange.client.resource);
            assert_eq!(exchange.state.resource_kind, RESOURCE_STREAM);
            assert_eq!(exchange.state.resource_flags & FLAG_REFERENCED, 0);
            assert_eq!(exchange.close_listener.value_kind, VALUE_UNDEFINED);
            assert_eq!(exchange.close_client.value_kind, VALUE_UNDEFINED);
            assert_eq!(exchange.close_accepted.value_kind, VALUE_UNDEFINED);
            assert!(exchange.requests.into_iter().all(|request| request != 0));
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(exchange.listener.drops, 1);
            assert_eq!(exchange.accepted.drops, 1);
            assert_eq!(exchange.client.drops, 1);
            assert_eq!(exchange.write.drops, 1);
            assert_eq!(exchange.read.drops, 1);
            assert_eq!(exchange.unref.drops, 1);
            assert_eq!(exchange.state.drops, 1);
            assert_eq!(exchange.close_listener.drops, 1);
            assert_eq!(exchange.close_client.drops, 1);
            assert_eq!(exchange.close_accepted.drops, 1);
        }

        #[test]
        fn active_host_op_submission_needs_no_provider_handle() {
            struct Submission {
                observation: *mut HostObservation,
                request: *mut u64,
            }

            unsafe extern "C" fn submit(context: *mut c_void) -> i32 {
                if context.is_null() {
                    return INVALID_ARGUMENT;
                }
                // SAFETY: the test owns this context for the complete realm entry.
                let submission = unsafe { &mut *context.cast::<Submission>() };
                let path = b"definitely-missing-kiln-active-op";
                let operation = HostOpView {
                    operation: OP_STAT,
                    flags: 1,
                    resource: 0,
                    argument: 0,
                    text: path.as_ptr(),
                    text_len: path.len() as u64,
                    bytes: ptr::null(),
                    bytes_len: 0,
                };
                // SAFETY: the frame is copied synchronously and both targets outlive drive.
                unsafe {
                    start_test_active_host_op(
                        &operation,
                        submission.observation,
                        submission.request,
                    )
                }
            }

            let mut handle = 0;
            let mut observation = HostObservation::default();
            let mut request = 0;
            let mut submission = Submission {
                observation: &raw mut observation,
                request: &raw mut request,
            };
            // SAFETY: all callback storage remains live through realm drive.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(handle, Some(submit), (&raw mut submission).cast::<c_void>())
                },
                0
            );
            assert_ne!(request, 0);
            assert_eq!(
                unsafe { kiln_realm_drive(handle, ptr::null_mut(), ptr::null_mut()) },
                0
            );
            assert_ne!(observation.status, 0);
            assert_eq!(observation.drops, 1);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        #[test]
        fn async_read_delivers_structured_error_and_drops_continuation() {
            let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-native-missing-{suffix}.txt"));
            let mut handle = 0;
            let mut request = 0;
            let mut observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut submission = ReadSubmission {
                handle: 0,
                path: path.to_string_lossy().as_bytes().to_vec(),
                observation: &raw mut observation,
                request: &raw mut request,
            };
            // SAFETY: all callback state remains live through the complete request drive.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            submission.handle = handle;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_read),
                        (&raw mut submission).cast::<c_void>(),
                    )
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(observation.status, 1);
            assert_eq!(observation.request, request);
            assert_eq!(observation.error_code, b"ENOENT");
            assert_eq!(observation.drops, 1);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        #[test]
        fn async_read_cancellation_prevents_resume_and_releases_continuation() {
            let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-native-cancel-{suffix}.txt"));
            assert!(fs::write(&path, b"canceled").is_ok());
            let mut handle = 0;
            let mut request = 0;
            let mut observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut submission = ReadSubmission {
                handle: 0,
                path: path.to_string_lossy().as_bytes().to_vec(),
                observation: &raw mut observation,
                request: &raw mut request,
            };
            // SAFETY: all callback state remains live through cancellation, drive, and teardown.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            submission.handle = handle;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_and_cancel),
                        (&raw mut submission).cast::<c_void>(),
                    )
                },
                0
            );
            assert_ne!(request, 0);
            assert_eq!(observation.drops, 1);
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(observation.status, -1);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert!(fs::remove_file(path).is_ok());
        }

        #[test]
        fn realm_destruction_releases_pending_async_continuations() {
            let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-native-drop-{suffix}.txt"));
            assert!(fs::write(&path, b"dropped").is_ok());
            let mut handle = 0;
            let mut request = 0;
            let mut observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut submission = ReadSubmission {
                handle: 0,
                path: path.to_string_lossy().as_bytes().to_vec(),
                observation: &raw mut observation,
                request: &raw mut request,
            };
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            submission.handle = handle;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_read),
                        (&raw mut submission).cast::<c_void>(),
                    )
                },
                0
            );
            assert_ne!(request, 0);
            assert_eq!(observation.drops, 0);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(observation.drops, 1);
            assert_eq!(observation.status, -1);
            assert!(fs::remove_file(path).is_ok());
        }

        #[test]
        fn async_reads_use_submission_order_when_ready_together() {
            let first_suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let second_suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let first = std::env::temp_dir().join(format!("kiln-native-order-{first_suffix}.txt"));
            let second =
                std::env::temp_dir().join(format!("kiln-native-order-{second_suffix}.txt"));
            assert!(fs::write(&first, b"first").is_ok());
            assert!(fs::write(&second, b"second").is_ok());
            let mut handle = 0;
            let mut first_request = 0;
            let mut second_request = 0;
            let mut first_observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut second_observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut first_submission = ReadSubmission {
                handle: 0,
                path: first.to_string_lossy().as_bytes().to_vec(),
                observation: &raw mut first_observation,
                request: &raw mut first_request,
            };
            let mut second_submission = ReadSubmission {
                handle: 0,
                path: second.to_string_lossy().as_bytes().to_vec(),
                observation: &raw mut second_observation,
                request: &raw mut second_request,
            };
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            first_submission.handle = handle;
            second_submission.handle = handle;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_read),
                        (&raw mut first_submission).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(submit_read),
                        (&raw mut second_submission).cast::<c_void>(),
                    )
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(first_observation.sequence, 0);
            assert_eq!(second_observation.sequence, 1);
            assert_eq!(first_observation.bytes, b"first");
            assert_eq!(second_observation.bytes, b"second");
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert!(fs::remove_file(first).is_ok());
            assert!(fs::remove_file(second).is_ok());
        }

        #[test]
        fn async_request_rejects_cross_realm_and_stale_cancellation() {
            let suffix = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kiln-native-realm-{suffix}.txt"));
            assert!(fs::write(&path, b"realm").is_ok());
            let mut first_handle = 0;
            let mut second_handle = 0;
            assert_eq!(unsafe { kiln_realm_create(&raw mut first_handle) }, 0);
            assert_eq!(unsafe { kiln_realm_create(&raw mut second_handle) }, 0);
            let mut request = 0;
            let mut observation = ReadObservation {
                status: -1,
                request: 0,
                sequence: 0,
                bytes: Vec::new(),
                error_code: Vec::new(),
                drops: 0,
            };
            let mut submission = ReadSubmission {
                handle: first_handle,
                path: path.to_string_lossy().as_bytes().to_vec(),
                observation: &raw mut observation,
                request: &raw mut request,
            };
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_handle,
                        Some(submit_read),
                        (&raw mut submission).cast::<c_void>(),
                    )
                },
                0
            );
            let mut cross_realm_result = -1;
            let mut cross_realm = CancelSubmission {
                handle: first_handle,
                request,
                result: &raw mut cross_realm_result,
            };
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        second_handle,
                        Some(cancel_request),
                        (&raw mut cross_realm).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(cross_realm_result, INVALID_ARGUMENT);
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        first_handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(observation.status, 0);
            let mut stale_result = -1;
            let mut stale = CancelSubmission {
                handle: first_handle,
                request,
                result: &raw mut stale_result,
            };
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        first_handle,
                        Some(cancel_request),
                        (&raw mut stale).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(stale_result, 1);
            assert_eq!(kiln_realm_destroy(first_handle), 0);
            assert_eq!(kiln_realm_destroy(second_handle), 0);
            assert!(fs::remove_file(path).is_ok());
        }

        #[test]
        fn realm_clock_is_visible_only_while_entered() {
            let mut handle = 0;
            let mut now = f64::NAN;
            // SAFETY: handle and clock slots remain live for their calls.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(reads_realm_clock),
                        (&raw mut now).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(now, 0.0);
            // SAFETY: the slot is live, but no Realm is active now.
            assert_eq!(unsafe { kiln_realm_now(&raw mut now) }, INVALID_ARGUMENT);
            assert_eq!(kiln_realm_destroy(handle), 0);
        }

        unsafe extern "C" fn arms_short_timer(context: *mut c_void) -> i32 {
            // SAFETY: the test supplies one live `Instant` slot for the timer to stamp.
            let fired = context.cast::<Option<std::time::Instant>>();
            unsafe extern "C" fn stamp(
                context: *mut c_void,
                _host: *mut c_void,
                _thrown: *mut c_void,
            ) -> i32 {
                // SAFETY: the slot outlives the drive that runs this timer.
                unsafe {
                    context
                        .cast::<Option<std::time::Instant>>()
                        .write(Some(std::time::Instant::now()))
                };
                0
            }
            unsafe extern "C" fn no_drop(_: *mut c_void) {}
            let mut id = 0;
            // SAFETY: the callbacks and the slot stay live through the drive.
            unsafe {
                kiln_realm_set_timeout(30.0, Some(stamp), Some(no_drop), fired.cast(), &raw mut id)
            }
        }

        #[test]
        fn wall_clock_realm_reads_the_host_and_fires_timers_on_it() -> Result<(), Box<dyn Error>> {
            let mut handle = 0;
            let mut now = f64::NAN;
            let mut fired: Option<std::time::Instant> = None;
            // SAFETY: every slot outlives the calls that use it.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(kiln_realm_use_wall_clock(0), INVALID_ARGUMENT);
            assert_eq!(kiln_realm_use_wall_clock(handle), 0);
            let before = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as f64;
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(reads_realm_clock),
                        (&raw mut now).cast::<c_void>(),
                    )
                },
                0
            );
            assert!(
                now >= before,
                "wall clock {now} reads before the test started {before}"
            );
            let armed = std::time::Instant::now();
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(arms_short_timer),
                        (&raw mut fired).cast::<c_void>(),
                    )
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            let fired = fired.ok_or("the timer ran during the drive")?;
            // Virtual time would have jumped the 30 ms; wall time waits for them.
            assert!(fired.duration_since(armed) >= std::time::Duration::from_millis(30));
            assert_eq!(kiln_realm_destroy(handle), 0);
            Ok(())
        }

        #[test]
        fn rust_panics_become_runtime_errors_before_reaching_c() {
            assert_eq!(
                ffi(|| std::panic::resume_unwind(Box::new("realm panic"))),
                RUNTIME_ERROR
            );
        }

        #[test]
        fn opaque_work_uses_realm_ordering_and_drops_every_payload_once() {
            let mut handle = 0;
            let mut observed = (Vec::<u8>::new(), 0_usize);
            // SAFETY: handle and context are live for the calls that use them.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            assert_eq!(
                unsafe {
                    kiln_realm_enter(
                        handle,
                        Some(schedule_recorded_work),
                        (&raw mut observed).cast::<c_void>(),
                    )
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            // SAFETY: both opaque callback-context slots remain live for the complete drive.
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                0
            );
            assert_eq!(observed.0, [5, 6, 1, 2, 3, 4]);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(observed.1, 6);
        }

        #[test]
        fn thrown_next_tick_drops_its_payload_before_drive_returns() {
            let mut handle = 0;
            let mut observed = (Vec::<u8>::new(), 0_usize);
            // SAFETY: handle and context are live for the calls that use them.
            assert_eq!(unsafe { kiln_realm_create(&raw mut handle) }, 0);
            unsafe extern "C" fn schedule(context: *mut c_void) -> i32 {
                if context.is_null() {
                    return INVALID_ARGUMENT;
                }
                // SAFETY: the test keeps this context live through Realm entry and drive.
                let (order, drops) = unsafe { &mut *context.cast::<(Vec<u8>, usize)>() };
                let work = recorded_work(7, order, drops);
                // SAFETY: callbacks match the transferred allocation.
                let status = unsafe {
                    kiln_realm_queue_next_tick(
                        Some(fail_recorded_work),
                        Some(drop_recorded_work),
                        work,
                    )
                };
                if status != 0 {
                    // SAFETY: ownership did not transfer on failure.
                    unsafe { drop_recorded_work(work) };
                }
                status
            }
            assert_eq!(
                unsafe {
                    kiln_realm_enter(handle, Some(schedule), (&raw mut observed).cast::<c_void>())
                },
                0
            );
            let mut host = 0_u8;
            let mut thrown = 0_u8;
            // SAFETY: both opaque callback-context slots remain live for the complete drive.
            assert_eq!(
                unsafe {
                    kiln_realm_drive(
                        handle,
                        (&raw mut host).cast::<c_void>(),
                        (&raw mut thrown).cast::<c_void>(),
                    )
                },
                RUNTIME_ERROR
            );
            assert_eq!(observed.0, [7]);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(observed.1, 1);
        }
    }

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
        (realm, kiln_realm_symbol_for, realm_symbol_for) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut u64) -> i32 = kiln_realm_symbol_for;
        };
        (realm, kiln_realm_symbol_key_for, realm_symbol_key_for) => {
            const _: unsafe extern "C" fn(u64, *mut *const u8, *mut u64) -> i32 = kiln_realm_symbol_key_for;
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
        (realm, kiln_realm_gc_attach, realm_gc_attach) => {
            const _: unsafe extern "C" fn(*const GcHooks, *mut c_void, *mut *mut c_void) -> i32 = kiln_realm_gc_attach;
        };
        (realm, kiln_realm_gc_register_roots, realm_gc_register_roots) => {
            const _: unsafe extern "C" fn(Option<GcRootsFn>, *mut c_void, u32) -> i32 = kiln_realm_gc_register_roots;
        };
        (realm, kiln_realm_gc_unregister_roots, realm_gc_unregister_roots) => {
            const _: unsafe extern "C" fn(Option<GcRootsFn>, *mut c_void) -> i32 = kiln_realm_gc_unregister_roots;
        };
        (realm, kiln_realm_gc_mark_roots, realm_gc_mark_roots) => {
            const _: unsafe extern "C" fn(*mut c_void) -> i32 = kiln_realm_gc_mark_roots;
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
        (realm, kiln_realm_clear_timer, realm_clear_timer) => {
            const _: extern "C" fn(u64) -> i32 = kiln_realm_clear_timer;
        };
        (realm, kiln_realm_drive, realm_drive) => {
            const _: unsafe extern "C" fn(u64, *mut c_void, *mut c_void) -> i32 = kiln_realm_drive;
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
        ($group:ident, $name:ident, $signature:ident) => {};
    }

    kiln_native_number_abi::provider_abi_registry!(assert_provider_export);
}

/// The allocator the regexp engine runs on.
///
/// `malloc` and `free`, because the generated artifact this is linked into already has them and a
/// second allocator would be a second heap in one process. Alignment above `max_align_t` goes
/// through `posix_memalign`, which is the only part `malloc` cannot promise.
#[cfg(all(not(test), any(feature = "regexp", feature = "bigint")))]
mod heap {
    use core::alloc::{GlobalAlloc, Layout};

    unsafe extern "C" {
        fn malloc(size: usize) -> *mut core::ffi::c_void;
        fn free(pointer: *mut core::ffi::c_void);
        fn posix_memalign(
            out: *mut *mut core::ffi::c_void,
            alignment: usize,
            size: usize,
        ) -> core::ffi::c_int;
    }

    /// What `malloc` already guarantees on every target this compiles for.
    const MALLOC_ALIGNMENT: usize = 16;

    struct CHeap;

    // SAFETY: every allocation is returned by `malloc` or `posix_memalign` and freed by `free`,
    // which is the contract both halves of libc's allocator keep.
    unsafe impl GlobalAlloc for CHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGNMENT {
                // SAFETY: a nonzero size is guaranteed by the `Layout` contract.
                return unsafe { malloc(layout.size()) }.cast::<u8>();
            }
            let mut out = core::ptr::null_mut();
            // SAFETY: `out` is a valid slot for the returned pointer.
            let status = unsafe { posix_memalign(&raw mut out, layout.align(), layout.size()) };
            if status == 0 {
                out.cast::<u8>()
            } else {
                core::ptr::null_mut()
            }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
            // SAFETY: the pointer came from `alloc` above and is freed exactly once.
            unsafe { free(pointer.cast::<core::ffi::c_void>()) };
        }
    }

    #[global_allocator]
    static HEAP: CHeap = CHeap;
}

/// Format `value` using ECMAScript `Number::toString` spelling.
///
/// On success the function returns the byte length excluding a trailing NUL and writes that NUL.
/// A null output, zero capacity, or a buffer without room for both the bytes and terminator returns
/// `-1` without modifying the output.
///
/// # Safety
///
/// When `buffer` is non-null, it must name storage valid for writes of `capacity` bytes for the
/// duration of this call. The storage must not overlap memory used by this function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_number_to_js_string(
    value: f64,
    buffer: *mut u8,
    capacity: usize,
) -> i32 {
    if buffer.is_null() || capacity == 0 {
        return INVALID_ARGUMENT;
    }

    let mut formatted = [0_u8; kiln_number_core::MAX_FORMATTED_LEN];
    let Ok(length) = kiln_number_core::format_number(value, &mut formatted) else {
        return INVALID_ARGUMENT;
    };
    let Some(required) = length.checked_add(1) else {
        return INVALID_ARGUMENT;
    };
    if capacity < required {
        return INVALID_ARGUMENT;
    }
    let Some(bytes) = formatted.get(..length) else {
        return INVALID_ARGUMENT;
    };
    let Ok(result) = i32::try_from(length) else {
        return INVALID_ARGUMENT;
    };

    // SAFETY: the caller contract guarantees `capacity` writable bytes, the check above proves
    // `length + 1 <= capacity`, and `formatted` is disjoint stack storage.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, length);
        buffer.add(length).write(0);
    }
    result
}

/// Parse an ECMAScript `StringNumericLiteral` from a byte span.
///
/// `(NULL, 0)` is the empty string and produces positive zero. A null pointer with a nonzero length,
/// a span too large for Rust's slice contract, invalid UTF-8, or malformed input produces `NaN`.
///
/// # Safety
///
/// When `bytes` is non-null and `length` is nonzero, it must name readable storage of exactly
/// `length` bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_string_to_number(bytes: *const u8, length: usize) -> f64 {
    if length == 0 {
        return 0.0;
    }
    if bytes.is_null() || length > isize::MAX as usize {
        return f64::NAN;
    }

    // SAFETY: the caller contract supplies `length` readable bytes, null was rejected, `u8` has
    // alignment one, and the explicit bound satisfies `slice::from_raw_parts`' size requirement.
    let input = unsafe { core::slice::from_raw_parts(bytes, length) };
    kiln_number_core::string_to_number(input)
}

macro_rules! assert_root_provider_export {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        $(assert_root_provider_export!($group, $name, $signature);)*
    };
    (root, kiln_number_to_js_string, number_to_js_string) => {
        const _: unsafe extern "C" fn(f64, *mut u8, usize) -> i32 = kiln_number_to_js_string;
    };
    (root, kiln_string_to_number, string_to_number) => {
        const _: unsafe extern "C" fn(*const u8, usize) -> f64 = kiln_string_to_number;
    };
    ($group:ident, $name:ident, $signature:ident) => {};
}

kiln_native_number_abi::provider_abi_registry!(assert_root_provider_export);

#[cfg(feature = "regexp")]
mod regexp {
    use super::INVALID_ARGUMENT;

    /// A compiled pattern, and the last match it produced.
    ///
    /// The match is kept because a capture group's *name* is only reachable through one -- `regress`
    /// clones the names into each match rather than exposing them on the pattern. Keeping it also
    /// means the C side reads a result with two calls instead of marshalling a variable-length
    /// structure across the boundary.
    pub struct KilnRegex {
        compiled: regress::Regex,
        last: Option<regress::Match>,
    }

    /// Compile `pattern` with `flags`.
    ///
    /// Returns 0 and writes the handle on success, and -1 for a pattern the engine rejects -- which
    /// the caller turns into a SyntaxError, because that is what `new RegExp` with the same text
    /// produces.
    ///
    /// # Safety
    ///
    /// `pattern` and `flags` must name readable UTF-8 spans of the given lengths, and `out_handle`
    /// must be a writable slot. The handle belongs to the caller and is released by
    /// [`kiln_regexp_free`].
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_compile(
        pattern: *const u8,
        pattern_len: usize,
        flags: *const u8,
        flags_len: usize,
        out_handle: *mut *mut KilnRegex,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees both spans are readable for their lengths; a null
        // pointer with a zero length is the empty span, which `from_raw_parts` requires be dangling
        // rather than null.
        let (pattern, flags) = unsafe { (span(pattern, pattern_len), span(flags, flags_len)) };
        let (Some(pattern), Some(flags)) = (pattern, flags) else {
            return INVALID_ARGUMENT;
        };
        let (Ok(pattern), Ok(flags)) = (core::str::from_utf8(pattern), core::str::from_utf8(flags))
        else {
            return INVALID_ARGUMENT;
        };
        let Ok(compiled) = regress::Regex::with_flags(pattern, flags) else {
            return INVALID_ARGUMENT;
        };
        let handle = alloc::boxed::Box::new(KilnRegex {
            compiled,
            last: None,
        });
        // SAFETY: `out_handle` is a writable slot by the caller contract.
        unsafe { out_handle.write(alloc::boxed::Box::into_raw(handle)) };
        0
    }

    /// Release a handle from [`kiln_regexp_compile`].
    ///
    /// # Safety
    ///
    /// `handle` must have come from [`kiln_regexp_compile`] and must not be used again.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_free(handle: *mut KilnRegex) {
        if handle.is_null() {
            return;
        }
        // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw` above.
        drop(unsafe { alloc::boxed::Box::from_raw(handle) });
    }

    /// Search `subject` from `start`, both counted in UTF-16 code units.
    ///
    /// Returns 1 for a match, 0 for none, and -1 for an invalid argument. On a match, up to
    /// `out_capacity` pairs of `(start, end)` offsets are written to `out_ranges`, one pair per group
    /// beginning with the whole match; a group that did not participate writes `(-1, -1)`.
    ///
    /// The offsets are UTF-16 code units because that is what the language counts in (ADR 0022), and
    /// matching over UTF-16 is what makes them so without a conversion.
    ///
    /// # Safety
    ///
    /// `handle` must be live, `subject` must name a readable span of `subject_len` units, and
    /// `out_ranges` must be writable for `out_capacity * 2` values.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_search(
        handle: *mut KilnRegex,
        subject: *const u16,
        subject_len: usize,
        start: usize,
        out_ranges: *mut i64,
        out_capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live and uniquely borrowed here.
        let state = unsafe { &mut *handle };
        if start > subject_len {
            state.last = None;
            return 0;
        }
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Some(units) = (unsafe { units(subject, subject_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(found) = state.compiled.find_from_utf16(units, start).next() else {
            state.last = None;
            return 0;
        };
        let mut written = 0usize;
        for group in found.groups() {
            if written >= out_capacity || out_ranges.is_null() {
                break;
            }
            let (from, to) = match group {
                Some(range) => (as_offset(range.start), as_offset(range.end)),
                // Not zero: a group that did not participate is `undefined`, which is a different
                // answer from one that matched nothing.
                None => (-1, -1),
            };
            // SAFETY: `written` is below `out_capacity`, and the caller promised twice that many
            // writable values.
            unsafe {
                out_ranges.add(written.wrapping_mul(2)).write(from);
                out_ranges
                    .add(written.wrapping_mul(2).wrapping_add(1))
                    .write(to);
            }
            written = written.wrapping_add(1);
        }
        state.last = Some(found);
        1
    }

    /// How many groups the last match reported, including the whole match as group zero.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_group_count(handle: *const KilnRegex) -> usize {
        if handle.is_null() {
            return 0;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let state = unsafe { &*handle };
        state
            .last
            .as_ref()
            .map_or(0, |found| found.groups().count())
    }

    /// The name of capture group `index` from the last match, as UTF-8 without a terminator.
    ///
    /// Returns the byte length written, 0 for a group with no name, and -1 when there is no last match
    /// or the buffer is too small.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `buffer` must be writable for `capacity` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_group_name(
        handle: *const KilnRegex,
        index: usize,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let state = unsafe { &*handle };
        let Some(found) = state.last.as_ref() else {
            return INVALID_ARGUMENT;
        };
        // `named_groups` counts from the first capture group, where the caller counts from the whole
        // match -- so group zero has no name by construction and the rest are shifted by one.
        let Some(wanted) = index.checked_sub(1) else {
            return 0;
        };
        let Some((name, _)) = found.named_groups().nth(wanted) else {
            return 0;
        };
        let bytes = name.as_bytes();
        let Ok(length) = i32::try_from(bytes.len()) else {
            return INVALID_ARGUMENT;
        };
        if bytes.len() > capacity || buffer.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the length is within the capacity the caller promised, and `name` is disjoint from
        // the caller's buffer.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        length
    }

    /// A byte span, or `None` when the pointer contract is violated.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` bytes.
    unsafe fn span<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// The same, for the UTF-16 subject.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` units.
    unsafe fn units<'a>(pointer: *const u16, length: usize) -> Option<&'a [u16]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize / 2 {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// An offset that fits the boundary type, saturating rather than wrapping.
    fn as_offset(value: usize) -> i64 {
        i64::try_from(value).unwrap_or(i64::MAX)
    }

    macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (regexp, kiln_regexp_compile, regexp_compile) => {
            const _: unsafe extern "C" fn(*const u8, usize, *const u8, usize, *mut *mut KilnRegex) -> i32 = kiln_regexp_compile;
        };
        (regexp, kiln_regexp_free, regexp_free) => {
            const _: unsafe extern "C" fn(*mut KilnRegex) = kiln_regexp_free;
        };
        (regexp, kiln_regexp_search, regexp_search) => {
            const _: unsafe extern "C" fn(*mut KilnRegex, *const u16, usize, usize, *mut i64, usize) -> i32 = kiln_regexp_search;
        };
        (regexp, kiln_regexp_group_count, regexp_group_count) => {
            const _: unsafe extern "C" fn(*const KilnRegex) -> usize = kiln_regexp_group_count;
        };
        (regexp, kiln_regexp_group_name, regexp_group_name) => {
            const _: unsafe extern "C" fn(*const KilnRegex, usize, *mut u8, usize) -> i32 = kiln_regexp_group_name;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

    kiln_native_number_abi::provider_abi_registry!(assert_provider_export);
}

/// Arbitrary-precision integers, exactly as the interpreter holds them.
///
/// A BigInt exists to be exact, so the one thing this must never do is lose a digit. Handing the
/// work to `num-bigint` -- the crate the interpreter already links -- is what makes "the two
/// engines agree" a property of the build rather than of the test suite.
//
// The crate denies `arithmetic_side_effects` because a fixed-width overflow is a silent wrong
// answer. Arbitrary-precision arithmetic has no such failure -- it grows instead -- and the two
// operations that *can* fail, division by zero and an exponent no machine can hold, are checked
// before they run.
#[expect(clippy::arithmetic_side_effects)]
#[cfg(feature = "bigint")]
mod bigint {
    use super::INVALID_ARGUMENT;
    use alloc::boxed::Box;
    use num_bigint::BigInt;
    use num_traits::{FromPrimitive, Pow, Signed, ToPrimitive, Zero};

    /// An operation the language defines but this cannot perform: division by zero, or an
    /// exponent no machine could represent. The caller turns it into a RangeError.
    const OUT_OF_RANGE: i32 = -2;

    /// The binary operators, numbered. Stable only within one generated unit, like every other
    /// operation code in the emitted runtime.
    const ADD: u32 = 1;
    const SUBTRACT: u32 = 2;
    const MULTIPLY: u32 = 3;
    const DIVIDE: u32 = 4;
    const REMAINDER: u32 = 5;
    const EXPONENTIATE: u32 = 6;
    const BITWISE_AND: u32 = 7;
    const BITWISE_OR: u32 = 8;
    const BITWISE_XOR: u32 = 9;
    const SHIFT_LEFT: u32 = 10;
    const SHIFT_RIGHT: u32 = 11;

    /// How far `<<` may widen a BigInt, in bits.
    ///
    /// The language puts no bound on the type -- an implementation is expected to run out of
    /// memory eventually -- but "out of memory" is not an outcome a program can catch, and
    /// `1n << (2n ** 64n)` is a count no allocation could ever satisfy. The caller turns this
    /// refusal into the `RangeError` every engine raises there.
    const MAX_SHIFT: u64 = 1 << 24;

    /// Parse a digit string in `radix`, with an optional leading sign.
    ///
    /// # Safety
    ///
    /// `text` must name a readable span of `len` bytes when `len` is nonzero.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_parse(
        text: *const u8,
        len: usize,
        radix: u32,
        out: *mut *mut BigInt,
    ) -> i32 {
        if out.is_null() || !(2..=36).contains(&radix) {
            return INVALID_ARGUMENT;
        }
        if len == 0 {
            return INVALID_ARGUMENT;
        }
        if text.is_null() || len > isize::MAX as usize {
            return INVALID_ARGUMENT;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        let bytes = unsafe { core::slice::from_raw_parts(text, len) };
        let Ok(digits) = core::str::from_utf8(bytes) else {
            return INVALID_ARGUMENT;
        };
        let Some(parsed) = BigInt::parse_bytes(digits.trim().as_bytes(), radix) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(parsed))) };
        0
    }

    /// `BigInt(n)`, which requires an integral finite number: rounding here would be the one thing
    /// a BigInt exists to prevent.
    ///
    /// # Safety
    ///
    /// `out` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_from_double(value: f64, out: *mut *mut BigInt) -> i32 {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        // `trunc` rather than `fract`: `num_traits` brings its own `fract` into scope here and
        // the two disagree about nothing except which one the compiler picks.
        if !value.is_finite() || value != libm_trunc(value) {
            return OUT_OF_RANGE;
        }
        let Some(converted) = BigInt::from_f64(value) else {
            return OUT_OF_RANGE;
        };
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(converted))) };
        0
    }

    /// # Safety
    ///
    /// `handle` must have come from one of the constructors above and must not be used again.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_free(handle: *mut BigInt) {
        if handle.is_null() {
            return;
        }
        // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw`.
        drop(unsafe { Box::from_raw(handle) });
    }

    /// The nearest double, which is what a comparison against a number needs -- and is lossy by
    /// definition, which is why nothing else uses it.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_to_double(handle: *const BigInt) -> f64 {
        if handle.is_null() {
            return f64::NAN;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        unsafe { &*handle }.to_f64().unwrap_or(f64::NAN)
    }

    /// The exact value as a 128-bit integer, split into a signed top half and an unsigned bottom
    /// half because C99 has no portable one -- the same pair `kiln_temporal_now` takes.
    ///
    /// Exact or nothing: [`OUT_OF_RANGE`] when the value does not fit, never a saturated or
    /// truncated one. This is what `Temporal.Instant.fromEpochNanoseconds` needs, and an instant
    /// off by 2^128 nanoseconds would be a wrong answer that prints rather than a caught error.
    ///
    /// # Safety
    ///
    /// `handle` must be live and both out-parameters writable slots.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_to_i128(
        handle: *const BigInt,
        out_high: *mut i64,
        out_low: *mut u64,
    ) -> i32 {
        if handle.is_null() || out_high.is_null() || out_low.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let Some(value) = unsafe { &*handle }.to_i128() else {
            return OUT_OF_RANGE;
        };
        // SAFETY: both out-parameters are writable slots by the caller contract.
        unsafe {
            out_high.write((value >> 64) as i64);
            out_low.write(value as u64);
        }
        0
    }

    /// Whether the value is zero, which is the whole of a BigInt's truthiness.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_is_zero(handle: *const BigInt) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        i32::from(unsafe { &*handle }.is_zero())
    }

    /// `-1`, `0` or `1`, or `-2` when either handle is null.
    ///
    /// # Safety
    ///
    /// Both handles must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_compare(left: *const BigInt, right: *const BigInt) -> i32 {
        if left.is_null() || right.is_null() {
            return OUT_OF_RANGE;
        }
        // SAFETY: the caller contract guarantees both handles are live.
        let (left, right) = unsafe { (&*left, &*right) };
        match left.cmp(right) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        }
    }

    /// The value written in `radix`, as UTF-8 without a terminator, or the required length as a
    /// negative number when the buffer is too small.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `buffer` must be writable for `capacity` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_to_string(
        handle: *const BigInt,
        radix: u32,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if handle.is_null() || !(2..=36).contains(&radix) {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let text = unsafe { &*handle }.to_str_radix(radix);
        let bytes = text.as_bytes();
        let Ok(length) = i32::try_from(bytes.len()) else {
            return INVALID_ARGUMENT;
        };
        if bytes.len() > capacity || buffer.is_null() {
            // Negative, so the caller can size a buffer and ask again rather than guessing.
            return length.saturating_neg();
        }
        // SAFETY: the length is within the capacity the caller promised.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        length
    }

    /// One binary operation.
    ///
    /// # Safety
    ///
    /// Both handles must be live and `out` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_binary(
        operation: u32,
        left: *const BigInt,
        right: *const BigInt,
        out: *mut *mut BigInt,
    ) -> i32 {
        if left.is_null() || right.is_null() || out.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees both handles are live.
        let (left, right) = unsafe { (&*left, &*right) };
        let produced = match operation {
            ADD => left + right,
            SUBTRACT => left - right,
            MULTIPLY => left * right,
            // Division truncates toward zero and by zero is a RangeError, neither of which
            // matches what a number would do.
            DIVIDE | REMAINDER => {
                if right.is_zero() {
                    return OUT_OF_RANGE;
                }
                if operation == DIVIDE {
                    left / right
                } else {
                    left % right
                }
            }
            EXPONENTIATE => {
                if right.is_negative() {
                    return OUT_OF_RANGE;
                }
                let Some(exponent) = right.to_u32() else {
                    return OUT_OF_RANGE;
                };
                Pow::pow(left.clone(), exponent)
            }
            BITWISE_AND => left & right,
            BITWISE_OR => left | right,
            BITWISE_XOR => left ^ right,
            // One operation with a sign rather than two: a negative count shifts the other way,
            // so `1n << -1n` is `1n >> 1n`. A widening shift past the bound is refused before
            // anything is allocated; a narrowing one is clamped to the value's own width, since
            // shifting further only converges on 0 or -1.
            SHIFT_LEFT | SHIFT_RIGHT => {
                let negative = right.is_negative();
                let widening = (operation == SHIFT_LEFT) != negative;
                let count = if negative {
                    -right.clone()
                } else {
                    right.clone()
                };
                match count.to_u64() {
                    Some(count) if widening => {
                        if count > MAX_SHIFT {
                            return OUT_OF_RANGE;
                        }
                        left.clone() << usize::try_from(count).unwrap_or(usize::MAX)
                    }
                    Some(count) => {
                        let clamped = count.min(left.bits().saturating_add(1));
                        left.clone() >> usize::try_from(clamped).unwrap_or(usize::MAX)
                    }
                    None if widening => return OUT_OF_RANGE,
                    None => left.clone() >> usize::MAX,
                }
            }
            _ => return INVALID_ARGUMENT,
        };
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(produced))) };
        0
    }

    /// `trunc` without pulling in a math library: a double's integral part.
    fn libm_trunc(value: f64) -> f64 {
        // `as i64` would saturate for anything large, which is exactly the case that matters.
        if value.abs() >= 9_007_199_254_740_992.0 {
            return value;
        }
        #[expect(clippy::cast_possible_truncation)]
        let whole = value as i64;
        #[expect(clippy::cast_precision_loss)]
        let whole = whole as f64;
        whole
    }

    /// `-x`, which is the only unary operation a BigInt has that is not a comparison.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `out` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_bigint_negate(
        handle: *const BigInt,
        out: *mut *mut BigInt,
    ) -> i32 {
        if handle.is_null() || out.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let negated = -(unsafe { &*handle }).clone();
        // SAFETY: `out` is a writable slot by the caller contract.
        unsafe { out.write(Box::into_raw(Box::new(negated))) };
        0
    }

    macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (bigint, kiln_bigint_parse, bigint_parse) => {
            const _: unsafe extern "C" fn(*const u8, usize, u32, *mut *mut BigInt) -> i32 = kiln_bigint_parse;
        };
        (bigint, kiln_bigint_from_double, bigint_from_double) => {
            const _: unsafe extern "C" fn(f64, *mut *mut BigInt) -> i32 = kiln_bigint_from_double;
        };
        (bigint, kiln_bigint_free, bigint_free) => {
            const _: unsafe extern "C" fn(*mut BigInt) = kiln_bigint_free;
        };
        (bigint, kiln_bigint_to_double, bigint_to_double) => {
            const _: unsafe extern "C" fn(*const BigInt) -> f64 = kiln_bigint_to_double;
        };
        (bigint, kiln_bigint_to_string, bigint_to_string) => {
            const _: unsafe extern "C" fn(*const BigInt, u32, *mut u8, usize) -> i32 = kiln_bigint_to_string;
        };
        (bigint, kiln_bigint_binary, bigint_binary) => {
            const _: unsafe extern "C" fn(u32, *const BigInt, *const BigInt, *mut *mut BigInt) -> i32 = kiln_bigint_binary;
        };
        (bigint, kiln_bigint_negate, bigint_negate) => {
            const _: unsafe extern "C" fn(*const BigInt, *mut *mut BigInt) -> i32 = kiln_bigint_negate;
        };
        (bigint, kiln_bigint_compare, bigint_compare) => {
            const _: unsafe extern "C" fn(*const BigInt, *const BigInt) -> i32 = kiln_bigint_compare;
        };
        (bigint, kiln_bigint_is_zero, bigint_is_zero) => {
            const _: unsafe extern "C" fn(*const BigInt) -> i32 = kiln_bigint_is_zero;
        };
        (bigint, kiln_bigint_to_i128, bigint_to_i128) => {
            const _: unsafe extern "C" fn(*const BigInt, *mut i64, *mut u64) -> i32 = kiln_bigint_to_i128;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

    kiln_native_number_abi::provider_abi_registry!(assert_provider_export);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn formatter_translates_capacity_and_nul_contracts_without_partial_writes() {
        let mut exact = [b'X'; 6];
        // SAFETY: `exact` is writable for its full reported capacity.
        let length = unsafe { kiln_number_to_js_string(1.234, exact.as_mut_ptr(), exact.len()) };
        assert_eq!(length, 5);
        assert_eq!(&exact, b"1.234\0");

        let mut short = [b'X'; 5];
        // SAFETY: `short` is writable for its full reported capacity.
        let length = unsafe { kiln_number_to_js_string(1.234, short.as_mut_ptr(), short.len()) };
        assert_eq!(length, INVALID_ARGUMENT);
        assert_eq!(&short, b"XXXXX");

        // SAFETY: a null pointer is explicitly accepted as an invalid argument and never read.
        assert_eq!(
            unsafe { kiln_number_to_js_string(1.0, core::ptr::null_mut(), 0) },
            INVALID_ARGUMENT
        );
    }

    #[test]
    fn parser_maps_null_empty_and_bounded_bytes_to_the_existing_contract() {
        // SAFETY: `(NULL, 0)` is explicitly the empty span.
        let empty = unsafe { kiln_string_to_number(core::ptr::null(), 0) };
        assert_eq!(empty.to_bits(), 0.0_f64.to_bits());

        // SAFETY: null with nonzero length is rejected before it can be read.
        assert!(unsafe { kiln_string_to_number(core::ptr::null(), 1) }.is_nan());

        let text = b"0x20000000000003";
        // SAFETY: `text` is readable for its reported length.
        let parsed = unsafe { kiln_string_to_number(text.as_ptr(), text.len()) };
        assert_eq!(parsed, 9_007_199_254_740_996.0);

        let malformed = [0xc0, 0xaf];
        // SAFETY: `malformed` is readable for its reported length.
        assert!(unsafe { kiln_string_to_number(malformed.as_ptr(), malformed.len()) }.is_nan());
    }
}

/// `Temporal`, over the same `kiln-temporal` adapter the interpreter links (ADR 0025, ADR 0029 §4).
///
/// The boundary keeps the discipline the regexp and BigInt providers set: the value is an opaque
/// handle, and nothing that is not a scalar crosses. That is cheaper here than anywhere else,
/// because `kiln-temporal` was already built with a scalar seam -- fields come back by *name* as
/// numbers, strings and flags, and options arrive as strings -- so one generic reader serves every
/// field of all eight types and the C side needs no per-field code.
///
/// The instant is always an argument, never read here. ADR 0015 makes the realm's clock virtual and
/// its zone UTC, and a provider that could reach the host's clock would be able to break that
/// invariant without anything failing to compile.
#[cfg(feature = "temporal")]
mod temporal {
    use super::INVALID_ARGUMENT;
    use kiln_temporal::{Fields, Kind, Options, TemporalValue};
    use std::cell::RefCell;

    /// A failed call left a message behind. Negative, so a caller testing `< 0` catches every one.
    ///
    /// All three classes are carried rather than flattened. `Temporal` picks between them on nearly
    /// every path -- a field out of range is a RangeError, malformed ISO text a SyntaxError, a
    /// wrong argument *type* a TypeError -- and the interpreter raises exactly what the engine
    /// chose, so collapsing any two here would make the two engines disagree about which error a
    /// program catches.
    const RANGE_ERROR: i32 = -2;
    const TYPE_ERROR: i32 = -3;
    const SYNTAX_ERROR: i32 = -4;

    /// The numeric field slots [`kiln_temporal_from_fields`] reads, in order: eraYear, year,
    /// month, day, hour, minute, second, millisecond, microsecond, nanosecond. NaN means absent,
    /// which is what lets one array carry an arbitrary subset without a presence mask beside it.
    const FIELD_SLOTS: usize = 10;

    /// The ten `Duration` components, in the order [`kiln_temporal::DurationFields`] declares them.
    const DURATION_SLOTS: usize = 10;

    /// The field string slots, in order: era, monthCode, offset, timeZone, calendar.
    ///
    /// They arrive as one concatenated buffer plus a length per slot, because five pointer/length
    /// pairs in a C signature is a shape nobody can call correctly twice. A length of -1 means the
    /// program did not supply the field at all, which is not the same as supplying `""`.
    ///
    /// Options are *not* here: they travel in [`OPTION_TEXT_SLOTS`], because `overflow` is not the
    /// only one a constructor reads -- a `ZonedDateTime` built from a bag also decides what a
    /// wall-clock time that exists twice means -- and one array an option can be read from is
    /// better than two.
    const TEXT_SLOTS: usize = 5;

    thread_local! {
        /// The message and class of the last failure on this thread.
        ///
        /// Carried out-of-band because the fallible calls already spend their return value on the
        /// error *class* -- and `Temporal` is unusually specific about that class, so flattening a
        /// RangeError into a TypeError would be a visible wrong answer rather than a lost detail.
        static LAST_ERROR: RefCell<(i32, String)> = const { RefCell::new((0, String::new())) };
    }

    fn remember(error: &kiln_temporal::Error) -> i32 {
        let code = match error.kind {
            kiln_temporal::ErrorKind::Type => TYPE_ERROR,
            kiln_temporal::ErrorKind::Syntax => SYNTAX_ERROR,
            kiln_temporal::ErrorKind::Range => RANGE_ERROR,
        };
        LAST_ERROR.with(|slot| *slot.borrow_mut() = (code, error.message.clone()));
        code
    }

    /// Remember a TypeError this side raised, rather than one the engine did.
    ///
    /// The engine's own constructors are private, and rightly so: the two cases this needs are
    /// argument-arity failures that never reach it, and inventing an error *class* here would be
    /// exactly the flattening `remember` exists to prevent. The class is the one the language
    /// gives a method called with too few arguments.
    fn reject_type(message: &str) -> i32 {
        LAST_ERROR.with(|slot| *slot.borrow_mut() = (TYPE_ERROR, message.to_owned()));
        TYPE_ERROR
    }

    /// An operation that needs a second `Temporal` value and did not get one.
    fn missing_operand() -> i32 {
        reject_type("this Temporal operation needs another Temporal value")
    }

    /// An operation that needs a property bag or a string argument and did not get one.
    fn missing_argument() -> i32 {
        reject_type("this Temporal operation needs an argument")
    }

    fn kind_of(code: u32) -> Option<Kind> {
        Some(match code {
            0 => Kind::PlainDate,
            1 => Kind::PlainTime,
            2 => Kind::PlainDateTime,
            3 => Kind::PlainYearMonth,
            4 => Kind::PlainMonthDay,
            5 => Kind::Duration,
            6 => Kind::Instant,
            7 => Kind::ZonedDateTime,
            _ => return None,
        })
    }

    fn code_of(kind: Kind) -> i32 {
        match kind {
            Kind::PlainDate => 0,
            Kind::PlainTime => 1,
            Kind::PlainDateTime => 2,
            Kind::PlainYearMonth => 3,
            Kind::PlainMonthDay => 4,
            Kind::Duration => 5,
            Kind::Instant => 6,
            Kind::ZonedDateTime => 7,
        }
    }

    /// A byte span, or `None` when the pointer contract is violated.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` bytes.
    unsafe fn span<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// An optional UTF-8 string argument: absent when the pointer is null.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` bytes.
    unsafe fn text<'a>(pointer: *const u8, length: usize) -> Result<Option<&'a str>, ()> {
        if pointer.is_null() {
            return Ok(None);
        }
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Some(bytes) = (unsafe { span(pointer, length) }) else {
            return Err(());
        };
        core::str::from_utf8(bytes).map(Some).map_err(|_| ())
    }

    /// Copy `value` into `buffer`, answering the length written, or the negative length it wanted.
    ///
    /// The retry protocol the BigInt provider uses: one negative answer tells the caller exactly
    /// how much to allocate, so a second call always succeeds.
    ///
    /// # Safety
    ///
    /// `buffer` must be writable for `capacity` bytes when `capacity` is non-zero.
    unsafe fn emit(value: &str, buffer: *mut u8, capacity: usize) -> i32 {
        let bytes = value.as_bytes();
        let Ok(length) = i32::try_from(bytes.len()) else {
            return INVALID_ARGUMENT;
        };
        if bytes.len() > capacity || buffer.is_null() {
            return length.checked_neg().unwrap_or(INVALID_ARGUMENT);
        }
        // SAFETY: `buffer` is writable for `capacity >= bytes.len()` by the caller contract, and
        // the source is a distinct allocation owned by this side.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        length
    }

    /// Split the packed text buffer into its slots.
    ///
    /// # Safety
    ///
    /// `text` must be readable for the sum of the non-negative lengths, and `lengths` for `count`
    /// values.
    unsafe fn unpack<'a, const N: usize>(
        text: *const u8,
        lengths: *const i64,
        count: usize,
    ) -> Option<[Option<&'a str>; N]> {
        if count != N || lengths.is_null() {
            return None;
        }
        // SAFETY: the caller contract guarantees `lengths` is readable for `count` values.
        let lengths = unsafe { core::slice::from_raw_parts(lengths, count) };
        let mut out: [Option<&str>; N] = [None; N];
        let mut offset = 0usize;
        for (slot, &length) in lengths.iter().enumerate() {
            if length < 0 {
                continue;
            }
            let length = usize::try_from(length).ok()?;
            let start = text.wrapping_add(offset);
            // SAFETY: the caller contract guarantees the concatenation is readable for the sum of
            // the non-negative lengths, and `offset` never passes that sum.
            let bytes = unsafe { span(start, length) }?;
            *out.get_mut(slot)? = Some(core::str::from_utf8(bytes).ok()?);
            offset = offset.checked_add(length)?;
        }
        Some(out)
    }

    /// The calendar and wall-clock bag, out of the numeric slots and the packed strings.
    ///
    /// Shared by [`kiln_temporal_from_fields`] and by `with`, which merges exactly the same bag
    /// into an existing value -- one reader, so the two cannot come to disagree about which slot
    /// carries `monthCode`.
    fn field_bag(slots: &[f64], strings: &[Option<&str>; TEXT_SLOTS]) -> Fields {
        let present = |index: usize| slots.get(index).copied().filter(|value| !value.is_nan());
        let owned = |index: usize| strings.get(index).copied().flatten().map(str::to_owned);
        Fields {
            era: owned(0),
            era_year: present(0),
            year: present(1),
            month: present(2),
            month_code: owned(1),
            day: present(3),
            hour: present(4),
            minute: present(5),
            second: present(6),
            millisecond: present(7),
            microsecond: present(8),
            nanosecond: present(9),
            offset: owned(2),
            time_zone: owned(3),
            calendar: owned(4),
        }
    }

    /// The ten duration components out of their numeric slots.
    fn duration_bag(slots: &[f64]) -> kiln_temporal::DurationFields {
        let present = |index: usize| slots.get(index).copied().filter(|value| !value.is_nan());
        kiln_temporal::DurationFields {
            years: present(0),
            months: present(1),
            weeks: present(2),
            days: present(3),
            hours: present(4),
            minutes: present(5),
            seconds: present(6),
            milliseconds: present(7),
            microseconds: present(8),
            nanoseconds: present(9),
        }
    }

    /// Build one of the eight types from a field bag the caller has already read.
    ///
    /// The caller does the reading, and in the order the specification names: property reads are
    /// observable, so a design that took the JavaScript object here would have moved observable
    /// behaviour behind a boundary that cannot express it. What crosses is the result.
    ///
    /// Returns 0 and writes the handle on success; -1 for a violated argument contract, and
    /// [`RANGE_ERROR`], [`TYPE_ERROR`] or [`SYNTAX_ERROR`] for a rejection whose message
    /// [`kiln_temporal_last_error`] then carries.
    ///
    /// # Safety
    ///
    /// `fields` must be readable for `field_count` doubles, `text` and `text_lengths` must satisfy
    /// [`unpack`], and `out_handle` must be a writable slot. The handle belongs to the caller and
    /// is released by [`kiln_temporal_free`].
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_from_fields(
        kind: u32,
        fields: *const f64,
        field_count: usize,
        text: *const u8,
        text_lengths: *const i64,
        text_count: usize,
        option_text: *const u8,
        option_text_lengths: *const i64,
        option_text_count: usize,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() || fields.is_null() || field_count != FIELD_SLOTS {
            return INVALID_ARGUMENT;
        }
        let Some(kind) = kind_of(kind) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees `fields` is readable for `field_count` doubles.
        let slots = unsafe { core::slice::from_raw_parts(fields, field_count) };
        // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
        let Some(strings) = (unsafe { unpack::<TEXT_SLOTS>(text, text_lengths, text_count) })
        else {
            return INVALID_ARGUMENT;
        };
        let bag = field_bag(slots, &strings);
        // SAFETY: the caller contract guarantees the packed options satisfy `unpack`.
        let Some(options) =
            (unsafe { option_strings(option_text, option_text_lengths, option_text_count) })
        else {
            return INVALID_ARGUMENT;
        };
        match kiln_temporal::from_fields(kind, &bag, &options) {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// `new Temporal.X(...)`: the *positional* constructors, which are not `from` with a bag.
    ///
    /// The difference is not cosmetic and is the reason this is a separate entry point rather
    /// than a flag on [`kiln_temporal_from_fields`]. A bag goes through the calendar: `from`
    /// resolves `monthCode`, honours an `overflow` option and defaults it to `constrain`. A
    /// constructor takes *ISO* components and rejects anything out of range --
    /// `new Temporal.PlainDate(2000, 300, 1)` is a RangeError where
    /// `Temporal.PlainDate.from({ year: 2000, month: 300, day: 1 })` is 2000-12-01. Routing one
    /// through the other would have been a silent wrong answer in both directions, so each
    /// reaches the engine function the interpreter already reaches: `kiln_temporal::plain_date`
    /// and its seven neighbours, the same linked engine under ADR 0025.
    ///
    /// `fields` carries the ten numeric slots in the [`kiln_temporal_from_fields`] layout, except
    /// for a `Duration`, whose ten are the [`kiln_temporal_duration_from_fields`] layout -- one
    /// array either way, because the caller already knows which of the eight it is building. NaN
    /// is absent, which is what carries the two optional reference components: a
    /// `PlainYearMonth`'s `referenceISODay` rides in the `day` slot and a `PlainMonthDay`'s
    /// `referenceISOYear` in the `year` slot, and leaving either out is not the same as passing
    /// zero. A missing *required* component stays NaN and the engine rejects it, which is the
    /// same RangeError the language gives.
    ///
    /// `epoch_high` and `epoch_low` are the signed top and unsigned bottom halves of the epoch
    /// nanosecond count the two exact types take, split because C99 has no portable 128-bit
    /// integer. They are ignored by the other six.
    ///
    /// # Safety
    ///
    /// `fields` must be readable for `field_count` doubles, `text` and `text_lengths` must
    /// satisfy [`unpack`], and `out_handle` must be a writable slot. The handle belongs to the
    /// caller and is released by [`kiln_temporal_free`].
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_construct(
        kind: u32,
        fields: *const f64,
        field_count: usize,
        text: *const u8,
        text_lengths: *const i64,
        text_count: usize,
        epoch_high: i64,
        epoch_low: u64,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() || fields.is_null() || field_count != FIELD_SLOTS {
            return INVALID_ARGUMENT;
        }
        let Some(kind) = kind_of(kind) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees `fields` is readable for `field_count` doubles.
        let slots = unsafe { core::slice::from_raw_parts(fields, field_count) };
        // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
        let Some(strings) = (unsafe { unpack::<TEXT_SLOTS>(text, text_lengths, text_count) })
        else {
            return INVALID_ARGUMENT;
        };
        let calendar = strings[4];
        let optional = |index: usize| {
            let value = slots[index];
            if value.is_nan() { None } else { Some(value) }
        };
        let epoch_nanoseconds = (i128::from(epoch_high) << 64) | i128::from(epoch_low);
        let built = match kind {
            Kind::PlainDate => kiln_temporal::plain_date(slots[1], slots[2], slots[3], calendar),
            Kind::PlainTime => kiln_temporal::plain_time(
                slots[4], slots[5], slots[6], slots[7], slots[8], slots[9],
            ),
            Kind::PlainDateTime => kiln_temporal::plain_date_time(
                slots[1], slots[2], slots[3], slots[4], slots[5], slots[6], slots[7], slots[8],
                slots[9], calendar,
            ),
            Kind::PlainYearMonth => {
                kiln_temporal::plain_year_month(slots[1], slots[2], optional(3), calendar)
            }
            Kind::PlainMonthDay => {
                kiln_temporal::plain_month_day(slots[2], slots[3], optional(1), calendar)
            }
            Kind::Duration => kiln_temporal::duration(&duration_bag(slots)),
            Kind::Instant => kiln_temporal::instant(epoch_nanoseconds),
            Kind::ZonedDateTime => match strings[3] {
                Some(zone) => kiln_temporal::zoned_date_time(epoch_nanoseconds, zone, calendar),
                // The one argument of the eight constructors that is neither numeric nor
                // optional. Rejected here rather than defaulted to UTC, which would build a
                // value the program did not ask for.
                None => {
                    return reject_type("new Temporal.ZonedDateTime needs a time zone");
                }
            },
        };
        match built {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// Build a `Temporal.Duration` from its ten components, NaN for a component left out.
    ///
    /// Separate from [`kiln_temporal_from_fields`] because the two bags are separate in the
    /// language: a duration has weeks and no month code, a date has a month code and no weeks.
    ///
    /// # Safety
    ///
    /// `fields` must be readable for `field_count` doubles and `out_handle` a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_duration_from_fields(
        fields: *const f64,
        field_count: usize,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() || fields.is_null() || field_count != DURATION_SLOTS {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees `fields` is readable for `field_count` doubles.
        let slots = unsafe { core::slice::from_raw_parts(fields, field_count) };
        let bag = duration_bag(slots);
        match kiln_temporal::duration_from_fields(&bag) {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// Parse one of the eight types out of its ISO text.
    ///
    /// # Safety
    ///
    /// `bytes` must be readable for `len`, and `out_handle` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_parse(
        kind: u32,
        bytes: *const u8,
        len: usize,
        option_text: *const u8,
        option_text_lengths: *const i64,
        option_text_count: usize,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(kind) = kind_of(kind) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Ok(Some(source)) = (unsafe { text(bytes, len) }) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the packed options satisfy `unpack`.
        let Some(options) =
            (unsafe { option_strings(option_text, option_text_lengths, option_text_count) })
        else {
            return INVALID_ARGUMENT;
        };
        // A `ZonedDateTime` string is the one `from` that has to decide what a wall-clock time
        // which does not exist, or exists twice, means -- so it reads the options the rest ignore.
        let parsed = if matches!(kind, Kind::ZonedDateTime) {
            kiln_temporal::parse_zoned_date_time(source, &options)
        } else {
            kiln_temporal::parse(kind, source)
        };
        match parsed {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// `Temporal.Now`, off the instant the realm supplies rather than any clock reachable here.
    ///
    /// The instant arrives split in two because C99 has no portable 128-bit integer: `high` is the
    /// signed top half and `low` the unsigned bottom half of the epoch nanosecond count. ADR 0015
    /// makes that count the realm's virtual clock, which starts at the epoch.
    ///
    /// `zone` is the optional argument `Temporal.Now.zonedDateTimeISO(zone)` takes; null means the
    /// realm's own, which ADR 0015 fixes at UTC.
    ///
    /// # Safety
    ///
    /// `out_handle` must be a writable slot, and a non-null `zone` readable for `zone_len` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_now(
        kind: u32,
        high: i64,
        low: u64,
        zone: *const u8,
        zone_len: usize,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(kind) = kind_of(kind) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Ok(zone) = (unsafe { text(zone, zone_len) }) else {
            return INVALID_ARGUMENT;
        };
        let epoch_nanoseconds = (i128::from(high) << 64) | i128::from(low);
        match kiln_temporal::now::read(kind, epoch_nanoseconds, zone) {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// `Temporal.Instant.fromEpochMilliseconds(ms)`.
    ///
    /// A separate entry point from [`kiln_temporal_from_fields`] because it is not a field bag:
    /// the argument is one number counted from the epoch, and the engine rejects a fractional or
    /// out-of-range one with the RangeError the language specifies rather than truncating it.
    ///
    /// # Safety
    ///
    /// `out_handle` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_instant_from_epoch_milliseconds(
        milliseconds: f64,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        match kiln_temporal::instant_from_epoch_milliseconds(milliseconds) {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// `Temporal.Instant.fromEpochNanoseconds(ns)`, where the argument is a `BigInt`.
    ///
    /// It arrives split the way [`kiln_temporal_now`]'s instant does, for the same reason: C99 has
    /// no portable 128-bit integer. The caller reads the two halves out of the BigInt provider,
    /// which is the only other place in this archive that holds a number this wide -- and the two
    /// providers stay independent features because nothing but scalars passes between them.
    ///
    /// # Safety
    ///
    /// `out_handle` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_instant_from_epoch_nanoseconds(
        high: i64,
        low: u64,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        let epoch_nanoseconds = (i128::from(high) << 64) | i128::from(low);
        match kiln_temporal::instant(epoch_nanoseconds) {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// Release a handle from any of the constructors.
    ///
    /// # Safety
    ///
    /// `handle` must have come from this module and must not be used again.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_free(handle: *mut TemporalValue) {
        if handle.is_null() {
            return;
        }
        // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw` above.
        drop(unsafe { Box::from_raw(handle) });
    }

    /// Which of the eight types `handle` is, as the code [`kiln_temporal_from_fields`] takes.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_kind(handle: *const TemporalValue) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        code_of(unsafe { &*handle }.kind())
    }

    /// One property, by the name the language spells it with.
    ///
    /// This is the call that makes the whole surface cheap: `kiln-temporal` answers every field of
    /// all eight types through one name-keyed reader, so the C side gets `year`, `daysInMonth` and
    /// `blank` alike without a line of per-field code.
    ///
    /// The answer says which shape it is: 0 no such property, 1 a number in `out_number`, 2 text
    /// written to `buffer` under the [`emit`] retry protocol, 3 a flag in `out_number`, 4 the
    /// property exists and is `undefined`, and 5 a value only a BigInt can hold -- `epochNanoseconds`
    /// -- whose exact decimal digits are written to `buffer` under the same retry protocol as
    /// shape 2, for the caller to hand to `kiln_bigint_parse`.
    ///
    /// Shape 5 travels as digits rather than as a BigInt handle on purpose: the two providers are
    /// independent features of one archive, and a nanosecond count is at most forty characters, so
    /// the decimal spelling is exact, is the spelling the language would print anyway, and costs
    /// this side no knowledge of what a BigInt is. Rounding it through the `f64` slot instead is
    /// the one thing that must never happen: a `Temporal` instant carries more precision than a
    /// double holds, and a rounded answer is a wrong answer that prints.
    ///
    /// # Safety
    ///
    /// `handle` must be live, `name` readable for `name_len`, `out_number` a writable slot, and
    /// `buffer` writable for `capacity` bytes when `capacity` is non-zero.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_get(
        handle: *const TemporalValue,
        name: *const u8,
        name_len: usize,
        out_number: *mut f64,
        buffer: *mut u8,
        capacity: usize,
        out_length: *mut i32,
    ) -> i32 {
        if handle.is_null() || out_number.is_null() || out_length.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Ok(Some(name)) = (unsafe { text(name, name_len) }) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the handle is live.
        let Some(field) = unsafe { &*handle }.get(name) else {
            return 0;
        };
        match field {
            kiln_temporal::Field::Number(number) => {
                // SAFETY: `out_number` is a writable slot by the caller contract.
                unsafe { out_number.write(number) };
                1
            }
            kiln_temporal::Field::Text(value) => {
                // SAFETY: `buffer` is writable for `capacity` by the caller contract.
                let written = unsafe { emit(&value, buffer, capacity) };
                // SAFETY: `out_length` is a writable slot by the caller contract.
                unsafe { out_length.write(written) };
                2
            }
            kiln_temporal::Field::Flag(flag) => {
                // SAFETY: `out_number` is a writable slot by the caller contract.
                unsafe { out_number.write(if flag { 1.0 } else { 0.0 }) };
                3
            }
            kiln_temporal::Field::Undefined => 4,
            kiln_temporal::Field::Integer128(integer) => {
                // SAFETY: `buffer` is writable for `capacity` by the caller contract.
                let written = unsafe { emit(&integer.to_string(), buffer, capacity) };
                // SAFETY: `out_length` is a writable slot by the caller contract.
                unsafe { out_length.write(written) };
                5
            }
        }
    }

    /// The value's own ISO text, which is what `toString` and `toJSON` both answer.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `buffer` writable for `capacity` bytes when `capacity` is
    /// non-zero.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_to_string(
        handle: *const TemporalValue,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        match unsafe { &*handle }.to_temporal_string(&Options::default()) {
            // SAFETY: `buffer` is writable for `capacity` by the caller contract.
            Ok(text) => unsafe { emit(&text, buffer, capacity) },
            Err(error) => remember(&error),
        }
    }

    /// Order two values of the same type: -1, 0 or 1, as `compare` answers.
    ///
    /// Two durations are the exception, and they take the options: whether one month is longer
    /// than thirty days depends on *which* month, so `Temporal.Duration.compare` needs a
    /// `relativeTo` and refuses without one rather than picking an average month.
    ///
    /// # Safety
    ///
    /// Both handles must be live, a non-null `relative_to` must be live, the packed options must
    /// satisfy [`unpack`], and `out_order` must be a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_compare(
        left: *const TemporalValue,
        right: *const TemporalValue,
        relative_to: *const TemporalValue,
        option_text: *const u8,
        option_text_lengths: *const i64,
        option_text_count: usize,
        out_order: *mut i32,
    ) -> i32 {
        if left.is_null() || right.is_null() || out_order.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees both handles are live.
        let (left, right) = unsafe { (&*left, &*right) };
        let ordered = if left.kind() == Kind::Duration && right.kind() == Kind::Duration {
            // SAFETY: the caller contract guarantees the packed options satisfy `unpack`.
            let Some(mut options) =
                (unsafe { option_strings(option_text, option_text_lengths, option_text_count) })
            else {
                return INVALID_ARGUMENT;
            };
            if !relative_to.is_null() {
                // SAFETY: the caller contract guarantees a non-null handle is live.
                options.relative_to = Some(unsafe { &*relative_to }.clone());
            }
            kiln_temporal::compare_durations(left, right, &options)
        } else {
            kiln_temporal::compare(left, right)
        };
        match ordered {
            Ok(order) => {
                // SAFETY: `out_order` is a writable slot by the caller contract.
                unsafe { out_order.write(order) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// The option-string slots, in the order the language reads them: calendarName,
    /// disambiguation, largestUnit, offset, overflow, roundingMode, smallestUnit, timeZoneName,
    /// unit.
    ///
    /// The reads themselves happen on the caller's side, because a getter on an options bag runs
    /// and the order it runs in is observable. What crosses is the strings that came back.
    const OPTION_TEXT_SLOTS: usize = 9;

    /// `[roundingIncrement, fractionalSecondDigits]`. NaN is absent; a negative digit count is the
    /// `"auto"` spelling, which is not the same as absent.
    const OPTION_NUMBER_SLOTS: usize = 2;

    /// Everything one operation needs, in one struct.
    ///
    /// A struct rather than twenty parameters: the arithmetic and conversion surface is
    /// name-keyed, so a single entry point serves all of it, and the alternative shape is a C
    /// signature nobody can call correctly twice.
    #[repr(C)]
    pub struct TemporalCall {
        /// The value the method was reached through.
        pub receiver: *const TemporalValue,
        /// The operation, by the name the language spells it with.
        pub name: *const u8,
        pub name_len: usize,
        /// The first argument when it is another `Temporal` value -- `add`'s duration, `until`'s
        /// endpoint, `withPlainTime`'s time. Null when the operation takes none.
        pub operand: *const TemporalValue,
        /// `relativeTo`, which decides whether a day is twenty-four hours.
        pub relative_to: *const TemporalValue,
        /// The first argument when it is text -- `withCalendar`'s calendar, `withTimeZone`'s zone.
        pub argument: *const u8,
        pub argument_len: usize,
        /// A field bag in the [`kiln_temporal_from_fields`] layout, or null for none. `with`
        /// merges it; `toPlainDate` on a year-month completes with it.
        pub fields: *const f64,
        pub field_count: usize,
        pub field_text: *const u8,
        pub field_text_lengths: *const i64,
        pub field_text_count: usize,
        /// The option numbers, [`OPTION_NUMBER_SLOTS`] of them, or null for none.
        pub option_numbers: *const f64,
        pub option_number_count: usize,
        /// The option strings, [`OPTION_TEXT_SLOTS`] of them packed as [`unpack`] takes them, or
        /// null for none.
        pub option_text: *const u8,
        pub option_text_lengths: *const i64,
        pub option_text_count: usize,
        /// Where a new value lands, when the answer is one.
        pub out_handle: *mut *mut TemporalValue,
        /// Where a number or a flag lands.
        pub out_number: *mut f64,
        /// Where text lands, under the [`emit`] retry protocol.
        pub out_buffer: *mut u8,
        pub out_capacity: usize,
        pub out_length: *mut i32,
    }

    /// The answer is a new value in `out_handle`.
    const ANSWER_HANDLE: i32 = 0;
    /// The answer is a number in `out_number`.
    const ANSWER_NUMBER: i32 = 1;
    /// The answer is text, written under the [`emit`] protocol with its length in `out_length`.
    const ANSWER_TEXT: i32 = 2;
    /// The answer is a flag in `out_number`.
    const ANSWER_FLAG: i32 = 3;
    /// This type has no such operation, and the caller refuses by name rather than answering.
    const ANSWER_UNKNOWN: i32 = 6;

    /// The nine option strings, out of the packed buffer the caller supplies.
    ///
    /// # Safety
    ///
    /// `text` and `lengths` must satisfy [`unpack`] for `count` slots.
    unsafe fn option_strings(
        text: *const u8,
        lengths: *const i64,
        count: usize,
    ) -> Option<Options> {
        let mut options = Options::default();
        if text.is_null() && count == 0 {
            return Some(options);
        }
        // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
        let strings = unsafe { unpack::<OPTION_TEXT_SLOTS>(text, lengths, count) }?;
        let owned = |index: usize| strings[index].map(str::to_owned);
        options.calendar_name = owned(0);
        options.disambiguation = owned(1);
        options.largest_unit = owned(2);
        // `offset` on an options bag is the conflict policy, not the field of the same name.
        options.offset_conflict = owned(3);
        options.overflow = owned(4);
        options.rounding_mode = owned(5);
        options.smallest_unit = owned(6);
        options.time_zone_name = owned(7);
        options.unit = owned(8);
        Some(options)
    }

    /// The whole option bag a call carries: the strings, the two numbers, and `relativeTo`.
    ///
    /// # Safety
    ///
    /// The call's option pointers must satisfy the contract [`kiln_temporal_invoke`] states.
    unsafe fn options_of(call: &TemporalCall) -> Option<Options> {
        // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
        let mut options = unsafe {
            option_strings(
                call.option_text,
                call.option_text_lengths,
                call.option_text_count,
            )
        }?;
        if !call.option_numbers.is_null() {
            if call.option_number_count != OPTION_NUMBER_SLOTS {
                return None;
            }
            // SAFETY: the caller contract guarantees the array is readable for its count.
            let numbers =
                unsafe { core::slice::from_raw_parts(call.option_numbers, OPTION_NUMBER_SLOTS) };
            if !numbers[0].is_nan() {
                options.rounding_increment = Some(numbers[0]);
            }
            if !numbers[1].is_nan() {
                options.fractional_second_digits = Some(if numbers[1] < 0.0 {
                    None
                } else {
                    Some(numbers[1].trunc() as u8)
                });
            }
        }
        if !call.relative_to.is_null() {
            // SAFETY: the caller contract guarantees a non-null handle is live.
            options.relative_to = Some(unsafe { &*call.relative_to }.clone());
        }
        Some(options)
    }

    /// The field bag the call carries, or `None` when it carries none.
    ///
    /// # Safety
    ///
    /// The call's field pointers must satisfy the contract [`kiln_temporal_invoke`] states.
    unsafe fn call_fields(call: &TemporalCall) -> Option<Option<Fields>> {
        if call.fields.is_null() {
            return Some(None);
        }
        if call.field_count != FIELD_SLOTS {
            return None;
        }
        // SAFETY: the caller contract guarantees the array is readable for its count.
        let slots = unsafe { core::slice::from_raw_parts(call.fields, FIELD_SLOTS) };
        // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
        let strings = unsafe {
            unpack::<TEXT_SLOTS>(
                call.field_text,
                call.field_text_lengths,
                call.field_text_count,
            )
        }?;
        Some(Some(field_bag(slots, &strings)))
    }

    /// The wall-clock half of a `PlainTime`, as the bag `toPlainDateTime` supplements with.
    ///
    /// Read through the same name-keyed reader the caller uses, so there is no second copy of the
    /// component list to drift from the first.
    fn time_supplement(time: &TemporalValue) -> Fields {
        let mut fields = Fields::default();
        let read = |name: &str| match time.get(name) {
            Some(kiln_temporal::Field::Number(number)) => Some(number),
            _ => None,
        };
        fields.hour = read("hour");
        fields.minute = read("minute");
        fields.second = read("second");
        fields.millisecond = read("millisecond");
        fields.microsecond = read("microsecond");
        fields.nanosecond = read("nanosecond");
        fields
    }

    /// One arithmetic or conversion operation, by the name the language spells it with.
    ///
    /// This is the other half of what [`kiln_temporal_get`] does for the readers: `kiln-temporal`
    /// already exposes the whole surface behind names and scalars, so one entry point carries
    /// `add`, `until`, `round`, `with`, `equals`, `total` and the eleven conversions without a
    /// line of per-method code on either side of the boundary.
    ///
    /// Answers [`ANSWER_HANDLE`], [`ANSWER_NUMBER`], [`ANSWER_TEXT`], [`ANSWER_FLAG`] or
    /// [`ANSWER_UNKNOWN`] -- the last of which is not a failure, but the signal that this type
    /// does not have this operation and the caller should refuse by name. A negative answer is
    /// [`INVALID_ARGUMENT`] or an error class whose message [`kiln_temporal_last_error`] carries.
    ///
    /// # Safety
    ///
    /// `call` must point at a live [`TemporalCall`] whose `receiver` is live, whose `name` is
    /// readable for `name_len`, whose non-null handles are live, whose non-null `argument` is
    /// readable for `argument_len`, whose non-null `fields`/`option_numbers` are readable for
    /// their counts, whose packed option and field text satisfies [`unpack`], and whose `out_*`
    /// slots are writable. A handle written to `out_handle` belongs to the caller.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_invoke(call: *const TemporalCall) -> i32 {
        if call.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the struct is live.
        let call = unsafe { &*call };
        if call.receiver.is_null()
            || call.out_handle.is_null()
            || call.out_number.is_null()
            || call.out_length.is_null()
        {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Ok(Some(name)) = (unsafe { text(call.name, call.name_len) }) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the handle is live.
        let receiver = unsafe { &*call.receiver };
        // SAFETY: the caller contract guarantees a non-null handle is live.
        let operand = if call.operand.is_null() {
            None
        } else {
            Some(unsafe { &*call.operand })
        };
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Ok(argument) = (unsafe { text(call.argument, call.argument_len) }) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the option pointers satisfy `options_of`.
        let Some(options) = (unsafe { options_of(call) }) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the field pointers satisfy `call_fields`.
        let Some(fields) = (unsafe { call_fields(call) }) else {
            return INVALID_ARGUMENT;
        };

        let produced: Result<TemporalValue, kiln_temporal::Error> = match name {
            "add" | "subtract" => match operand {
                None => return missing_operand(),
                Some(operand) if name == "add" => receiver.add(operand, &options),
                Some(operand) => receiver.subtract(operand, &options),
            },
            "until" | "since" => match operand {
                None => return missing_operand(),
                Some(operand) if name == "until" => receiver.until(operand, &options),
                Some(operand) => receiver.since(operand, &options),
            },
            "round" => receiver.round(&options),
            "with" => {
                let Some(bag) = fields else {
                    return missing_argument();
                };
                if receiver.kind() == Kind::Duration {
                    // A duration's bag is the *other* bag -- weeks and no month code -- and the
                    // caller packs it into the same numeric slots
                    // `kiln_temporal_duration_from_fields` reads.
                    // SAFETY: `call_fields` established the array is non-null and readable for
                    // `FIELD_SLOTS`, which is `DURATION_SLOTS`.
                    let slots = unsafe { core::slice::from_raw_parts(call.fields, FIELD_SLOTS) };
                    receiver.duration_with(&duration_bag(slots))
                } else {
                    receiver.with(&bag, &options)
                }
            }
            "negated" => receiver.negated(),
            "abs" => receiver.abs(),
            "startOfDay" => receiver.start_of_day(),
            "withCalendar" => match argument {
                None => return missing_argument(),
                Some(calendar) => receiver.with_calendar(calendar),
            },
            "withTimeZone" => match argument {
                None => return missing_argument(),
                Some(zone) => receiver.with_time_zone(zone),
            },
            "withPlainTime" => receiver.with_plain_time(operand),
            "toZonedDateTime" | "toZonedDateTimeISO" => match argument {
                None => return missing_argument(),
                Some(zone) => receiver.to_zoned_date_time(zone, operand, &options),
            },
            // `toPlainDate` means two different things: on a date-and-time it drops the time, and
            // on a year-month or a month-day it supplies the component that was missing.
            "toPlainDate"
                if matches!(receiver.kind(), Kind::PlainYearMonth | Kind::PlainMonthDay) =>
            {
                match fields {
                    None => return missing_argument(),
                    Some(fields) => receiver.to_plain_date_with(&fields),
                }
            }
            "toPlainDate" => receiver.to_kind(Kind::PlainDate, None),
            "toPlainTime" => receiver.to_kind(Kind::PlainTime, None),
            "toPlainDateTime" => {
                let supplement = match (operand, fields) {
                    (Some(time), _) if time.kind() == Kind::PlainTime => {
                        Some(time_supplement(time))
                    }
                    (_, Some(fields)) => Some(fields),
                    _ => None,
                };
                receiver.to_kind(Kind::PlainDateTime, supplement.as_ref())
            }
            "toPlainYearMonth" => receiver.to_kind(Kind::PlainYearMonth, None),
            "toPlainMonthDay" => receiver.to_kind(Kind::PlainMonthDay, None),
            "toInstant" => receiver.to_kind(Kind::Instant, None),
            "total" => {
                return match receiver.total(&options) {
                    Ok(number) => {
                        // SAFETY: `out_number` is a writable slot by the caller contract.
                        unsafe { call.out_number.write(number) };
                        ANSWER_NUMBER
                    }
                    Err(error) => remember(&error),
                };
            }
            "equals" => {
                let Some(operand) = operand else {
                    return missing_operand();
                };
                return match receiver.equals(operand) {
                    Ok(flag) => {
                        // SAFETY: `out_number` is a writable slot by the caller contract.
                        unsafe { call.out_number.write(if flag { 1.0 } else { 0.0 }) };
                        ANSWER_FLAG
                    }
                    Err(error) => remember(&error),
                };
            }
            "toString" => {
                return match receiver.to_temporal_string(&options) {
                    Ok(value) => {
                        // SAFETY: `out_buffer` is writable for `out_capacity` by the contract.
                        let written = unsafe { emit(&value, call.out_buffer, call.out_capacity) };
                        // SAFETY: `out_length` is a writable slot by the caller contract.
                        unsafe { call.out_length.write(written) };
                        ANSWER_TEXT
                    }
                    Err(error) => remember(&error),
                };
            }
            _ => return ANSWER_UNKNOWN,
        };
        match produced {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { call.out_handle.write(Box::into_raw(Box::new(value))) };
                ANSWER_HANDLE
            }
            Err(error) => remember(&error),
        }
    }

    /// `ToTemporalDate` and its siblings: narrow a value to another of the eight types.
    ///
    /// Not a method -- it is the abstract operation `Temporal.PlainDate.from(zonedDateTime)` runs,
    /// which is why it is its own entry point rather than a name through [`kiln_temporal_invoke`].
    /// A pair the language does not define is a TypeError, which the engine raises rather than
    /// this side guessing at the table.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `out_handle` a writable slot.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_to_kind(
        handle: *const TemporalValue,
        kind: u32,
        out_handle: *mut *mut TemporalValue,
    ) -> i32 {
        if handle.is_null() || out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(kind) = kind_of(kind) else {
            return INVALID_ARGUMENT;
        };
        // SAFETY: the caller contract guarantees the handle is live.
        match unsafe { &*handle }.to_kind(kind, None) {
            Ok(value) => {
                // SAFETY: `out_handle` is a writable slot by the caller contract.
                unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
                0
            }
            Err(error) => remember(&error),
        }
    }

    /// The message and class of the last failure on this thread.
    ///
    /// Writes the class to `out_kind` -- [`RANGE_ERROR`] or [`TYPE_ERROR`] -- and the message under
    /// the [`emit`] retry protocol. `Temporal` distinguishes the two classes on nearly every path,
    /// so the caller raises the one the engine chose rather than guessing.
    ///
    /// # Safety
    ///
    /// `out_kind` must be a writable slot and `buffer` writable for `capacity` bytes when
    /// `capacity` is non-zero.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_temporal_last_error(
        out_kind: *mut i32,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if out_kind.is_null() {
            return INVALID_ARGUMENT;
        }
        LAST_ERROR.with(|slot| {
            let (code, message) = &*slot.borrow();
            // SAFETY: `out_kind` is a writable slot by the caller contract.
            unsafe { out_kind.write(*code) };
            // SAFETY: `buffer` is writable for `capacity` by the caller contract.
            unsafe { emit(message, buffer, capacity) }
        })
    }

    macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (temporal, kiln_temporal_from_fields, temporal_from_fields) => {
            const _: unsafe extern "C" fn(u32, *const f64, usize, *const u8, *const i64, usize, *const u8, *const i64, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_from_fields;
        };
        (temporal, kiln_temporal_construct, temporal_construct) => {
            const _: unsafe extern "C" fn(u32, *const f64, usize, *const u8, *const i64, usize, i64, u64, *mut *mut TemporalValue) -> i32 = kiln_temporal_construct;
        };
        (temporal, kiln_temporal_duration_from_fields, temporal_duration_from_fields) => {
            const _: unsafe extern "C" fn(*const f64, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_duration_from_fields;
        };
        (temporal, kiln_temporal_parse, temporal_parse) => {
            const _: unsafe extern "C" fn(u32, *const u8, usize, *const u8, *const i64, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_parse;
        };
        (temporal, kiln_temporal_now, temporal_now) => {
            const _: unsafe extern "C" fn(u32, i64, u64, *const u8, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_now;
        };
        (temporal, kiln_temporal_instant_from_epoch_milliseconds, temporal_instant_ms) => {
            const _: unsafe extern "C" fn(f64, *mut *mut TemporalValue) -> i32 = kiln_temporal_instant_from_epoch_milliseconds;
        };
        (temporal, kiln_temporal_instant_from_epoch_nanoseconds, temporal_instant_ns) => {
            const _: unsafe extern "C" fn(i64, u64, *mut *mut TemporalValue) -> i32 = kiln_temporal_instant_from_epoch_nanoseconds;
        };
        (temporal, kiln_temporal_free, temporal_free) => {
            const _: unsafe extern "C" fn(*mut TemporalValue) = kiln_temporal_free;
        };
        (temporal, kiln_temporal_kind, temporal_kind) => {
            const _: unsafe extern "C" fn(*const TemporalValue) -> i32 = kiln_temporal_kind;
        };
        (temporal, kiln_temporal_get, temporal_get) => {
            const _: unsafe extern "C" fn(*const TemporalValue, *const u8, usize, *mut f64, *mut u8, usize, *mut i32) -> i32 = kiln_temporal_get;
        };
        (temporal, kiln_temporal_to_string, temporal_to_string) => {
            const _: unsafe extern "C" fn(*const TemporalValue, *mut u8, usize) -> i32 = kiln_temporal_to_string;
        };
        (temporal, kiln_temporal_compare, temporal_compare) => {
            const _: unsafe extern "C" fn(*const TemporalValue, *const TemporalValue, *const TemporalValue, *const u8, *const i64, usize, *mut i32) -> i32 = kiln_temporal_compare;
        };
        (temporal, kiln_temporal_invoke, temporal_invoke) => {
            const _: unsafe extern "C" fn(*const TemporalCall) -> i32 = kiln_temporal_invoke;
        };
        (temporal, kiln_temporal_to_kind, temporal_to_kind) => {
            const _: unsafe extern "C" fn(*const TemporalValue, u32, *mut *mut TemporalValue) -> i32 = kiln_temporal_to_kind;
        };
        (temporal, kiln_temporal_last_error, temporal_last_error) => {
            const _: unsafe extern "C" fn(*mut i32, *mut u8, usize) -> i32 = kiln_temporal_last_error;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

    kiln_native_number_abi::provider_abi_registry!(assert_provider_export);
}

/// `JSON.parse`'s reader, as a tape (ADR 0025's shape: nothing but scalars and byte ranges cross).
///
/// The grammar lives in `kiln-json`, which the interpreter links directly; this archive links the
/// same crate, so a generated program and the interpreter cannot disagree about what parses. The
/// emitted C walks the tape once and does only what must touch its heap: make strings, arrays and
/// objects, and define properties.
///
/// There is no handle and nothing to free. The tape and text live in the crate's per-thread
/// scratch, which a parse overwrites in place, so a call hands back pointers into it that stay
/// valid until the next parse on the same thread. One parse is in flight per thread by
/// construction -- the C walk allocates but never parses -- and the crate asserts it.
#[cfg(feature = "json")]
mod json {
    use super::INVALID_ARGUMENT;
    use kiln_json::{Node, ParseError};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    /// The text is not JSON. The caller raises the SyntaxError the language specifies. Shares the
    /// invalid-argument code deliberately: the two are the same answer to the emitted C, which
    /// never passes a bad argument, and a refusal here must never read as a fault that stops the
    /// program.
    const SYNTAX_ERROR: i32 = -1;
    /// The reader could not run at all: the text is longer than the tape indexes, or it panicked.
    const READER_FAILED: i32 = -2;

    /// Parse `len` bytes of UTF-8 at `bytes`.
    ///
    /// Returns 0 with the tape -- `out_count` nodes at `out_nodes`, in source order, whose string
    /// offsets index the `out_text_len` bytes at `out_text` -- or -1 for text that is not JSON,
    /// or -2 when the reader could not run. The four pointers stay valid until the next call on
    /// this thread and belong to the reader; the caller reads them and does not free them.
    ///
    /// # Safety
    ///
    /// `bytes` must be readable for `len` bytes (null is accepted only with a length of zero), and
    /// the four out pointers must be writable slots.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_json_parse(
        bytes: *const u8,
        len: usize,
        out_nodes: *mut *const Node,
        out_count: *mut usize,
        out_text: *mut *const u8,
        out_text_len: *mut usize,
    ) -> i32 {
        if (bytes.is_null() && len != 0)
            || out_nodes.is_null()
            || out_count.is_null()
            || out_text.is_null()
            || out_text_len.is_null()
        {
            return INVALID_ARGUMENT;
        }
        let source: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: the caller contract guarantees `bytes` is readable for `len` bytes.
            unsafe { core::slice::from_raw_parts(bytes, len) }
        };
        // A panic must not cross the C boundary: the archive's unwind policy makes one fatal to
        // the process, and this is `JSON.parse`, which a program wraps in `try`.
        let parsed = catch_unwind(AssertUnwindSafe(|| {
            kiln_json::parse(source, |tape| {
                let nodes = tape.nodes();
                let text = tape.text();
                (nodes.as_ptr(), nodes.len(), text.as_ptr(), text.len())
            })
        }));
        match parsed {
            Ok(Ok((nodes, count, text, text_len))) => {
                // SAFETY: the four slots are writable by the caller contract. The pointers are
                // into the crate's thread-local scratch, which outlives this call.
                unsafe {
                    out_nodes.write(nodes);
                    out_count.write(count);
                    out_text.write(text);
                    out_text_len.write(text_len);
                }
                0
            }
            Ok(Err(ParseError::Syntax { .. })) => SYNTAX_ERROR,
            Ok(Err(ParseError::TooLarge)) | Err(_) => READER_FAILED,
        }
    }

    macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (json, kiln_json_parse, json_parse) => {
            const _: unsafe extern "C" fn(*const u8, usize, *mut *const Node, *mut usize, *mut *const u8, *mut usize) -> i32 = kiln_json_parse;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

    kiln_native_number_abi::provider_abi_registry!(assert_provider_export);

    #[cfg(test)]
    mod tests {
        use super::*;
        use kiln_json::{TAG_ARRAY, TAG_NUMBER, TAG_OBJECT, TAG_STRING};

        struct Parsed {
            status: i32,
            tags: Vec<u32>,
            key: Vec<u8>,
        }

        fn parse(text: &str) -> Parsed {
            let mut nodes: *const Node = core::ptr::null();
            let mut count = 0usize;
            let mut arena: *const u8 = core::ptr::null();
            let mut arena_len = 0usize;
            let status = unsafe {
                kiln_json_parse(
                    text.as_ptr(),
                    text.len(),
                    &mut nodes,
                    &mut count,
                    &mut arena,
                    &mut arena_len,
                )
            };
            if status != 0 {
                return Parsed {
                    status,
                    tags: Vec::new(),
                    key: Vec::new(),
                };
            }
            let tape = unsafe { core::slice::from_raw_parts(nodes, count) };
            let text = unsafe { core::slice::from_raw_parts(arena, arena_len) };
            let key = tape
                .get(1)
                .filter(|node| node.tag == TAG_STRING)
                .and_then(|node| {
                    text.get(node.value as usize..(node.value as usize + node.len as usize))
                })
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            Parsed {
                status,
                tags: tape.iter().map(|node| node.tag).collect(),
                key,
            }
        }

        #[test]
        fn a_document_crosses_as_a_tape_into_this_threads_scratch() {
            let parsed = parse(r#"{"a":[1,"x"]}"#);
            assert_eq!(parsed.status, 0);
            assert_eq!(
                parsed.tags,
                vec![TAG_OBJECT, TAG_STRING, TAG_ARRAY, TAG_NUMBER, TAG_STRING]
            );
            assert_eq!(parsed.key, b"a");
            // The next parse overwrites the scratch in place; nothing was leaked or freed.
            let again = parse("[2,3]");
            assert_eq!(again.tags, vec![TAG_ARRAY, TAG_NUMBER, TAG_NUMBER]);
        }

        #[test]
        fn refusals_and_bad_arguments_are_told_apart() {
            assert_eq!(parse("{").status, SYNTAX_ERROR);
            assert_eq!(parse("").status, SYNTAX_ERROR);
            let mut nodes: *const Node = core::ptr::null();
            let status = unsafe {
                kiln_json_parse(
                    core::ptr::null(),
                    3,
                    &mut nodes,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                )
            };
            assert_eq!(status, INVALID_ARGUMENT);
        }
    }
}

/// `parallelMap`'s fork-join (ADR 0058, application 2).
///
/// The emitted C hands over the element and result arrays as opaque pointers, a chunk worker
/// over an index range, and a context pointer; Rust owns the thread pool, how the range is
/// split, and the floor below which splitting is not worth a wake-up. The elements are never
/// read here: they are the program's NaN-boxed values, and only the worker knows how to read
/// them, so nothing is copied or unboxed across the boundary and the worker runs against the
/// program's own arrays.
///
/// What a worker may do is decided in `kiln-backend-c` (`parallel.rs`): a body that reads its
/// `double` parameters and returns a `double`, reaching no host, heap or thread-local realm
/// state. That is what makes it safe to call on a thread that owns no realm.
#[cfg(feature = "parallel")]
mod parallel {
    use super::INVALID_ARGUMENT;
    use std::ffi::c_void;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::OnceLock;

    /// An emitted chunk worker: `[start, end)` of `in_` to the same range of `out`, with `ctx`
    /// handed back untouched.
    pub type ChunkFn = unsafe extern "C" fn(*const c_void, *mut c_void, usize, usize, *mut c_void);

    /// Below this many elements the whole range runs on the calling thread.
    ///
    /// Measured on an 18-core M-series laptop at load average 7 (another agent benchmarking
    /// alongside), `bench/non-io/parallel-map-large.mjs`'s body per element -- a square root, a
    /// `pow`, a remainder and a `floor`, ~9 ns inline -- pool against inline, us per call:
    ///
    /// | elements | pool | inline |
    /// | --- | --- | --- |
    /// | 2,048 | 14.9 | 14.8 |
    /// | 4,096 | 97.6 | 32.1 |
    /// | 8,192 | 107.8 | 63.1 |
    /// | 16,384 | 125.2 | 128.2 |
    /// | 65,536 | 197.4 | 543.6 |
    ///
    /// The fork-join costs 60-90 us of wake-up and join here, so it breaks even near 16k of that
    /// body and loses below; a lighter body (`x * 2 + 1`, ~1 ns) needs several times that. The
    /// floor sits at the heavy body's break-even, so `multicore-parallel.mjs`'s 5,000-element
    /// `x * 2 + 1` runs inline and does not regress, and a 2e6-element map is 8x faster.
    pub const SEQUENTIAL_FLOOR: usize = 16384;

    /// Elements per unit of work handed to the pool. Small enough that stealing evens out uneven
    /// bodies across workers, large enough that the per-chunk call is noise: 1,024 elements of
    /// a 15 ns body is 15 us of work against a ~50 ns call.
    pub const CHUNK: usize = 1024;

    /// The worker count: `KILN_PARALLELISM` when set to a positive integer, else the machine's
    /// available parallelism, else 1. `1` disables the pool entirely and every call runs inline,
    /// which is how a benchmark measures the sequential answer with the same binary.
    fn parallelism() -> usize {
        match std::env::var("KILN_PARALLELISM") {
            Ok(value) => match value.trim().parse::<usize>() {
                Ok(n) if n >= 1 => n,
                _ => 1,
            },
            Err(_) => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        }
    }

    /// The pool, built on first use: private to this archive, so it never shares Rayon's
    /// global pool with anything else linked into the process, and sized once per process.
    /// `None` when the answer is one worker.
    fn pool() -> Option<&'static rayon::ThreadPool> {
        static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
        POOL.get_or_init(|| {
            let threads = parallelism();
            if threads <= 1 {
                return None;
            }
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("kiln-parallel-{i}"))
                .build()
                .ok()
        })
        .as_ref()
    }

    /// The pointers a worker needs, made `Send + Sync` for the closure Rayon runs. The C side
    /// guarantees the ranges are disjoint per chunk and the arrays outlive the call.
    #[derive(Clone, Copy)]
    struct Job {
        in_: *const c_void,
        out: *mut c_void,
        ctx: *mut c_void,
        chunk: ChunkFn,
    }
    // SAFETY: each chunk writes only its own `[start, end)` of `out` and reads only its own range
    // of `in_`; `ctx` is written with a relaxed atomic store by the emitted worker and read after
    // the join. Nothing else on any worker touches these.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// One chunk. A method rather than a field read inside the closure, so the closure
        /// captures the `Sync` job as a whole and not its raw-pointer fields one by one.
        ///
        /// # Safety
        ///
        /// `[start, end)` must be within the arrays and disjoint from every other chunk in flight.
        unsafe fn run(&self, start: usize, end: usize) {
            // SAFETY: the caller's contract, which is `kiln_parallel_map`'s.
            unsafe { (self.chunk)(self.in_, self.out, start, end, self.ctx) }
        }
    }

    /// Run `chunk` over `[0, count)` of `in_` into `out`, split across the pool.
    ///
    /// Returns 0 when every chunk ran, -1 for a null worker, and -2 when a worker panicked, in
    /// which case some of `out` is unwritten and the caller takes its sequential path. The
    /// worker signals its own failure (a typed entry that bailed out) through `ctx`, which this
    /// function never reads.
    ///
    /// # Safety
    ///
    /// `in_` must be readable and `out` writable for `count` elements of whatever the worker
    /// reads and writes, both must stay valid for the call, and `chunk` must be a function that
    /// touches only its own range and thread-safe state.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_parallel_map(
        in_: *const c_void,
        count: usize,
        out: *mut c_void,
        chunk: Option<ChunkFn>,
        ctx: *mut c_void,
    ) -> i32 {
        let Some(chunk) = chunk else {
            return INVALID_ARGUMENT;
        };
        let job = Job {
            in_,
            out,
            ctx,
            chunk,
        };
        let pool = match pool() {
            Some(pool) if count >= SEQUENTIAL_FLOOR => pool,
            _ => {
                // SAFETY: the caller contract, for the whole range on this thread.
                unsafe { job.run(0, count) };
                return 0;
            }
        };
        let chunks = count.div_ceil(CHUNK);
        let ran = catch_unwind(AssertUnwindSafe(|| {
            pool.install(|| {
                use rayon::prelude::*;
                (0..chunks).into_par_iter().for_each(|k| {
                    let start = k.saturating_mul(CHUNK);
                    let end = start.saturating_add(CHUNK).min(count);
                    // SAFETY: `[start, end)` is this chunk's own disjoint range of a live array.
                    unsafe { job.run(start, end) };
                });
            })
        }));
        match ran {
            Ok(()) => 0,
            Err(_) => -2,
        }
    }

    macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (parallel, kiln_parallel_map, parallel_map) => {
            const _: unsafe extern "C" fn(*const c_void, usize, *mut c_void, Option<ChunkFn>, *mut c_void) -> i32 = kiln_parallel_map;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

    kiln_native_number_abi::provider_abi_registry!(assert_provider_export);

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        unsafe extern "C" fn double_it(
            in_: *const c_void,
            out: *mut c_void,
            start: usize,
            end: usize,
            ctx: *mut c_void,
        ) {
            let src = in_ as *const f64;
            let dst = out as *mut f64;
            for i in start..end {
                // SAFETY: the test owns both arrays and the ranges are within them.
                unsafe { dst.add(i).write(src.add(i).read() * 2.0) };
            }
            // SAFETY: `ctx` is the test's counter.
            unsafe {
                (*(ctx as *const AtomicUsize))
                    .fetch_add(end.saturating_sub(start), Ordering::Relaxed)
            };
        }

        fn run(count: usize) -> (i32, Vec<f64>, usize) {
            let input: Vec<f64> = (0..count).map(|i| i as f64).collect();
            let mut output = vec![0.0; count];
            let seen = AtomicUsize::new(0);
            // SAFETY: both vectors are live for the call and sized to `count`.
            let status = unsafe {
                kiln_parallel_map(
                    input.as_ptr().cast(),
                    count,
                    output.as_mut_ptr().cast(),
                    Some(double_it),
                    &seen as *const AtomicUsize as *mut c_void,
                )
            };
            (status, output, seen.load(Ordering::Relaxed))
        }

        #[test]
        fn every_element_is_visited_exactly_once_below_and_above_the_floor() {
            for count in [0, 1, SEQUENTIAL_FLOOR - 1, SEQUENTIAL_FLOOR, 10 * CHUNK + 7] {
                let (status, out, seen) = run(count);
                assert_eq!(status, 0, "count {count}");
                assert_eq!(seen, count, "count {count}");
                assert!(out.iter().enumerate().all(|(i, &v)| v == 2.0 * i as f64));
            }
        }

        #[test]
        fn a_null_worker_is_an_invalid_argument() {
            // SAFETY: nothing is dereferenced for a null worker.
            let status = unsafe {
                kiln_parallel_map(
                    std::ptr::null(),
                    3,
                    std::ptr::null_mut(),
                    None,
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(status, INVALID_ARGUMENT);
        }
    }
}

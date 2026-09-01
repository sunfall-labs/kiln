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

#[cfg(any(feature = "regexp", feature = "bigint"))]
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
    use core::ffi::c_void;
    use kiln_host_abi::{
        AsyncHostError, AsyncHostOp, AsyncHostValue, RequestId, ResourceId, ResourceKind,
    };
    use kiln_runtime::{
        ActiveHttpRuntime, HttpCloseMode, HttpStreamAction, Realm, RealmRuntime,
        RuntimeHttpClientEvent, RuntimeHttpClientRequest, RuntimeHttpEvent, RuntimeTurn, TimerId,
        WorkItem, duration_from_milliseconds,
        node_http_agent::{AgentDecision, AgentRelease, HttpOrigin},
    };
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};

    type Entry = unsafe extern "C" fn(*mut c_void) -> i32;
    type RunWork = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;
    type DropWork = unsafe extern "C" fn(*mut c_void);
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

    struct WorkOwner {
        pointer: *mut c_void,
        run: RunWork,
        drop: DropWork,
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

    impl NativeWork {
        fn new(pointer: *mut c_void, run: Option<RunWork>, drop: Option<DropWork>) -> Option<Self> {
            let (Some(run), Some(drop)) = (run, drop) else {
                return None;
            };
            if pointer.is_null() {
                return None;
            }
            Some(Self(Rc::new(WorkOwner { pointer, run, drop })))
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

    struct HttpEventPayload {
        owner: Rc<HttpEventOwner>,
        kind: u32,
        flags: u32,
        id: u64,
        status: u16,
        version_minor: u8,
        method: Vec<u8>,
        target: Vec<u8>,
        reason: Vec<u8>,
        headers: Vec<u8>,
        data: Vec<u8>,
    }

    impl HttpEventPayload {
        fn bytes(bytes: &[u8]) -> (*const u8, u64) {
            (
                bytes.as_ptr(),
                u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            )
        }

        fn view(&self) -> HttpEventView {
            let (method, method_len) = Self::bytes(&self.method);
            let (target, target_len) = Self::bytes(&self.target);
            let (reason, reason_len) = Self::bytes(&self.reason);
            let (headers, headers_len) = Self::bytes(&self.headers);
            let (data, data_len) = Self::bytes(&self.data);
            HttpEventView {
                kind: self.kind,
                flags: self.flags,
                id: self.id,
                status: self.status,
                version_minor: self.version_minor,
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
            }
        }
    }

    unsafe extern "C" fn run_http_event(
        pointer: *mut c_void,
        host: *mut c_void,
        thrown: *mut c_void,
    ) -> i32 {
        if pointer.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: NativeWork owns this boxed payload until the callback returns.
        let payload = unsafe { &*pointer.cast::<HttpEventPayload>() };
        let view = payload.view();
        // SAFETY: all borrowed spans remain owned by payload for this synchronous callback.
        unsafe { (payload.owner.run)(payload.owner.pointer, &raw const view, host, thrown) }
    }

    unsafe extern "C" fn drop_http_event(pointer: *mut c_void) {
        if !pointer.is_null() {
            // SAFETY: NativeWork invokes this exactly once for Box::into_raw below.
            unsafe { drop(Box::from_raw(pointer.cast::<HttpEventPayload>())) };
        }
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
        work: RefCell<Realm<NativeWork, NativeWork>>,
        continuations: RefCell<HashMap<RequestId, AsyncContinuation>>,
        http_connections: RefCell<HashMap<kiln_host_abi::ResourceId, NativeHttpConnection>>,
        http_requests: RefCell<HashMap<RequestId, kiln_host_abi::ResourceId>>,
        next_http_client_request_id: Cell<u64>,
        cancelled: Rc<RefCell<HashSet<RequestId>>>,
        queued: Rc<RefCell<HashSet<RequestId>>>,
        symbols: RefCell<HashMap<Vec<u8>, u64>>,
        symbol_keys: RefCell<HashMap<u64, Vec<u8>>>,
        entered: Cell<bool>,
    }

    impl NativeRealm {
        fn new() -> Result<Self, kiln_runtime::RuntimeBuildError> {
            Ok(Self {
                runtime: RefCell::new(RealmRuntime::new()?),
                work: RefCell::new(Realm::new()),
                continuations: RefCell::new(HashMap::new()),
                http_connections: RefCell::new(HashMap::new()),
                http_requests: RefCell::new(HashMap::new()),
                next_http_client_request_id: Cell::new(1),
                cancelled: Rc::new(RefCell::new(HashSet::new())),
                queued: Rc::new(RefCell::new(HashSet::new())),
                symbols: RefCell::new(HashMap::new()),
                symbol_keys: RefCell::new(HashMap::new()),
                entered: Cell::new(false),
            })
        }
    }

    struct ActiveRealm {
        realm: Rc<NativeRealm>,
    }

    impl ActiveRealm {
        fn enter(handle: u64, realm: Rc<NativeRealm>) -> Option<Self> {
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
            Some(Self { realm })
        }
    }

    impl Drop for ActiveRealm {
        fn drop(&mut self) {
            ACTIVE_REALM.with(|active| active.set(0));
            self.realm.entered.set(false);
        }
    }

    static NEXT_REALM: AtomicU64 = AtomicU64::new(1);
    static NEXT_SYMBOL: AtomicU64 = AtomicU64::new(1);

    std::thread_local! {
        static REALMS: RefCell<HashMap<u64, Rc<NativeRealm>>> = RefCell::new(HashMap::new());
        static ACTIVE_REALM: Cell<u64> = const { Cell::new(0) };
        static ACTIVE_RUNTIME: Cell<*mut RealmRuntime> = const { Cell::new(std::ptr::null_mut()) };
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
        ACTIVE_REALM.with(|active| realm(active.get()))
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

    fn run_in_runtime(realm: &NativeRealm, work: impl FnOnce() -> i32) -> i32 {
        let runtime = realm.runtime.as_ptr();
        let _active = ActiveRuntime::enter(runtime);
        // `RealmRuntime::enter` may synchronously call generated language code. That code can
        // re-enter the private active-runtime ABI (for example, an HTTP response callback can
        // call `ClientRequest.end`). Keep ownership explicit through `ACTIVE_RUNTIME`, but do not
        // hold the RefCell guard across the callback: active-runtime operations exclusively use
        // this pointer while `entered` prevents a nested realm drive.
        unsafe { &mut *runtime }.enter(work)
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

    fn flatten_headers(headers: &[(String, String)]) -> Vec<u8> {
        let capacity = headers.iter().fold(0usize, |total, (name, value)| {
            total
                .saturating_add(name.len())
                .saturating_add(value.len())
                .saturating_add(2)
        });
        let mut flattened = Vec::with_capacity(capacity);
        for (name, value) in headers {
            flattened.extend_from_slice(name.as_bytes());
            flattened.push(0);
            flattened.extend_from_slice(value.as_bytes());
            flattened.push(0);
        }
        flattened
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

    fn queue_http_event(realm: &NativeRealm, payload: HttpEventPayload) -> i32 {
        let pointer = Box::into_raw(Box::new(payload)).cast::<c_void>();
        let Some(work) = NativeWork::new(pointer, Some(run_http_event), Some(drop_http_event))
        else {
            // SAFETY: ownership was not transferred when construction failed.
            unsafe { drop_http_event(pointer) };
            return RUNTIME_ERROR;
        };
        realm.work.borrow_mut().queue_microtask(work);
        0
    }

    fn apply_http_actions(
        realm: &NativeRealm,
        resource: kiln_host_abi::ResourceId,
        actions: Vec<HttpStreamAction>,
        in_context: bool,
    ) -> i32 {
        for action in actions {
            match action {
                HttpStreamAction::Start(http_operation) => {
                    // An attached HTTP connection owns its accepted socket until Node explicitly
                    // closes or unreferences it. Reads are therefore referenced just like writes;
                    // otherwise the native drive can declare quiescence between pipelined
                    // requests and destroy a still-live keep-alive connection.
                    let operation = http_operation.clone().into_host_op();
                    let request = if in_context {
                        let Some(runtime) = active_runtime() else {
                            return INVALID_ARGUMENT;
                        };
                        // SAFETY: callbacks execute synchronously inside this realm's runtime.
                        unsafe { &mut *runtime }.start_host_op_in_context(operation.clone(), true)
                    } else {
                        realm
                            .runtime
                            .borrow_mut()
                            .start_host_op(operation.clone(), true)
                    };
                    let started = if in_context {
                        let Some(runtime) = active_runtime() else {
                            return INVALID_ARGUMENT;
                        };
                        // SAFETY: the same active runtime exclusively owns the HTTP connection.
                        unsafe { &mut *runtime }.http_connection_started(
                            resource,
                            &http_operation,
                            request,
                        )
                    } else {
                        realm.runtime.borrow_mut().http_connection_started(
                            resource,
                            &http_operation,
                            request,
                        )
                    };
                    if !started {
                        return RUNTIME_ERROR;
                    }
                    realm.http_requests.borrow_mut().insert(request, resource);
                }
                HttpStreamAction::Close(close_resource) => {
                    if close_resource.resource() != resource {
                        return RUNTIME_ERROR;
                    }
                    let request = if in_context {
                        let Some(runtime) = active_runtime() else {
                            return INVALID_ARGUMENT;
                        };
                        // SAFETY: callbacks execute synchronously inside this realm's runtime.
                        unsafe { &mut *runtime }
                            .start_http_connection_close_in_context(close_resource)
                    } else {
                        realm
                            .runtime
                            .borrow_mut()
                            .start_http_connection_close(close_resource)
                    };
                    let Some(request) = request else {
                        return RUNTIME_ERROR;
                    };
                    realm.http_requests.borrow_mut().insert(request, resource);
                }
                HttpStreamAction::Cancel(request) => {
                    realm.runtime.borrow_mut().cancel_host_op(request);
                    realm.http_requests.borrow_mut().remove(&request);
                }
                HttpStreamAction::Event(kiln_runtime::node_stream::StreamEvent::Close) => {
                    realm.http_connections.borrow_mut().remove(&resource);
                }
                HttpStreamAction::Event(_) => {}
                HttpStreamAction::Error(error) => {
                    eprintln!(
                        "native HTTP transport error on resource {}: {} ({} {})",
                        resource.get(),
                        error.message,
                        error.code,
                        error.syscall
                    );
                    return RUNTIME_ERROR;
                }
                HttpStreamAction::Data(_) => {
                    return RUNTIME_ERROR;
                }
            }
        }
        0
    }

    fn checkpoint_http_after_wake(realm: &NativeRealm) -> Result<usize, i32> {
        realm.runtime.borrow_mut().poll_http_tasks_after_wake();
        checkpoint_http(realm, false)
    }

    fn checkpoint_http(realm: &NativeRealm, in_context: bool) -> Result<usize, i32> {
        let mut event_count: usize = 0;
        if !in_context {
            // A response command can wake Hyper and make a pipelined request runnable without
            // another HostOps completion. Advance the one Tokio scheduler before inspecting
            // per-connection events, matching the interpreted adapter's checkpoint contract.
            realm.runtime.borrow_mut().poll_http_tasks();
        }
        let resources = realm
            .http_connections
            .borrow()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for resource in resources {
            loop {
                let actions = if in_context {
                    let Some(runtime) = active_runtime() else {
                        return Err(INVALID_ARGUMENT);
                    };
                    // SAFETY: this is the current realm runtime; this poll never nests block_on.
                    unsafe { &mut *runtime }.poll_http_connection_in_context(resource)
                } else {
                    // Pump this connection's already-selected bytes without re-driving the
                    // scheduler. `checkpoint_http` drives it once before this loop, and
                    // `poll_http_tasks_after_wake` drives it again after every iteration that
                    // did anything, so a drive here only repeats one of those -- once per open
                    // connection, after every microtask. That is where the server's cost went
                    // quadratic: idle connections each paid a full scheduler drive to report
                    // that they had nothing to say.
                    realm
                        .runtime
                        .borrow_mut()
                        .poll_http_connection_in_context(resource)
                };
                let Some(actions) = actions else { break };
                let had_actions = !actions.is_empty();
                let status = apply_http_actions(realm, resource, actions, in_context);
                if status != 0 {
                    return Err(status);
                }
                let (owner, role) = {
                    let connections = realm.http_connections.borrow();
                    let Some(connection) = connections.get(&resource) else {
                        break;
                    };
                    (Rc::clone(&connection.events), connection.role)
                };
                let mut payloads = Vec::new();
                let mut close_transport = false;
                let mut close_mode = HttpCloseMode::Graceful;
                match role {
                    HttpRole::Server => {
                        let events = if in_context {
                            let Some(runtime) = active_runtime() else {
                                return Err(INVALID_ARGUMENT);
                            };
                            // SAFETY: event extraction is synchronous on the active runtime.
                            unsafe { &mut *runtime }.take_http_events(resource)
                        } else {
                            realm.runtime.borrow_mut().take_http_events(resource)
                        };
                        for event in events {
                            let payload = match event {
                                RuntimeHttpEvent::RequestHead { id, request } => HttpEventPayload {
                                    owner: Rc::clone(&owner),
                                    kind: HTTP_EVENT_REQUEST_HEAD,
                                    flags: u32::from(request.keep_alive),
                                    id,
                                    status: 0,
                                    version_minor: u8::from(matches!(
                                        request.version,
                                        kiln_runtime::node_http::HttpVersion::Http11
                                    )),
                                    method: request.method.into_bytes(),
                                    target: request.target.into_bytes(),
                                    reason: Vec::new(),
                                    headers: flatten_headers(&request.headers),
                                    data: Vec::new(),
                                },
                                RuntimeHttpEvent::RequestData { id, data } => HttpEventPayload {
                                    owner: Rc::clone(&owner),
                                    kind: HTTP_EVENT_REQUEST_DATA,
                                    flags: 0,
                                    id,
                                    status: 0,
                                    version_minor: 0,
                                    method: Vec::new(),
                                    target: Vec::new(),
                                    reason: Vec::new(),
                                    headers: Vec::new(),
                                    data,
                                },
                                RuntimeHttpEvent::RequestEnd { id } => HttpEventPayload {
                                    owner: Rc::clone(&owner),
                                    kind: HTTP_EVENT_REQUEST_END,
                                    flags: 0,
                                    id,
                                    status: 0,
                                    version_minor: 0,
                                    method: Vec::new(),
                                    target: Vec::new(),
                                    reason: Vec::new(),
                                    headers: Vec::new(),
                                    data: Vec::new(),
                                },
                                RuntimeHttpEvent::Drain { id } => HttpEventPayload {
                                    owner: Rc::clone(&owner),
                                    kind: HTTP_EVENT_DRAIN,
                                    flags: 0,
                                    id,
                                    status: 0,
                                    version_minor: 0,
                                    method: Vec::new(),
                                    target: Vec::new(),
                                    reason: Vec::new(),
                                    headers: Vec::new(),
                                    data: Vec::new(),
                                },
                                RuntimeHttpEvent::Finish { id } => HttpEventPayload {
                                    owner: Rc::clone(&owner),
                                    kind: HTTP_EVENT_FINISH,
                                    flags: 0,
                                    id,
                                    status: 0,
                                    version_minor: 0,
                                    method: Vec::new(),
                                    target: Vec::new(),
                                    reason: Vec::new(),
                                    headers: Vec::new(),
                                    data: Vec::new(),
                                },
                                RuntimeHttpEvent::Error { id, message } => HttpEventPayload {
                                    owner: Rc::clone(&owner),
                                    kind: HTTP_EVENT_ERROR,
                                    flags: 0,
                                    id,
                                    status: 0,
                                    version_minor: 0,
                                    method: Vec::new(),
                                    target: Vec::new(),
                                    reason: Vec::new(),
                                    headers: Vec::new(),
                                    data: message.into_bytes(),
                                },
                                RuntimeHttpEvent::Closed => {
                                    // Server protocol completion can race the final response
                                    // write. Keep the default graceful mode so that write is
                                    // acknowledged before the portable resource is retired.
                                    close_transport = true;
                                    HttpEventPayload {
                                        owner: Rc::clone(&owner),
                                        kind: HTTP_EVENT_CLOSED,
                                        flags: 0,
                                        id: 0,
                                        status: 0,
                                        version_minor: 0,
                                        method: Vec::new(),
                                        target: Vec::new(),
                                        reason: Vec::new(),
                                        headers: Vec::new(),
                                        data: Vec::new(),
                                    }
                                }
                            };
                            payloads.push(payload);
                        }
                    }
                    HttpRole::Client => {
                        let events = if in_context {
                            let Some(runtime) = active_runtime() else {
                                return Err(INVALID_ARGUMENT);
                            };
                            // SAFETY: event extraction is synchronous on the active runtime.
                            unsafe { &mut *runtime }.take_http1_client_events(resource)
                        } else {
                            realm
                                .runtime
                                .borrow_mut()
                                .take_http1_client_events(resource)
                        };
                        for event in events {
                            let (id, kind, flags, status, reason, headers, data) = match event {
                                RuntimeHttpClientEvent::Drain { id } => (
                                    id,
                                    HTTP_EVENT_DRAIN,
                                    0,
                                    0,
                                    Vec::new(),
                                    Vec::new(),
                                    Vec::new(),
                                ),
                                RuntimeHttpClientEvent::Finish { id } => (
                                    id,
                                    HTTP_EVENT_FINISH,
                                    0,
                                    0,
                                    Vec::new(),
                                    Vec::new(),
                                    Vec::new(),
                                ),
                                RuntimeHttpClientEvent::ResponseHead {
                                    id,
                                    status,
                                    reason,
                                    headers,
                                } => (
                                    id,
                                    HTTP_EVENT_RESPONSE_HEAD,
                                    0,
                                    status,
                                    reason.into_bytes(),
                                    flatten_headers(&headers),
                                    Vec::new(),
                                ),
                                RuntimeHttpClientEvent::ResponseData { id, data } => (
                                    id,
                                    HTTP_EVENT_RESPONSE_DATA,
                                    0,
                                    0,
                                    Vec::new(),
                                    Vec::new(),
                                    data,
                                ),
                                RuntimeHttpClientEvent::ResponseEnd { id, reusable } => (
                                    id,
                                    HTTP_EVENT_RESPONSE_END,
                                    u32::from(reusable),
                                    0,
                                    Vec::new(),
                                    Vec::new(),
                                    Vec::new(),
                                ),
                                RuntimeHttpClientEvent::Error { id, message } => (
                                    id,
                                    HTTP_EVENT_ERROR,
                                    0,
                                    0,
                                    Vec::new(),
                                    Vec::new(),
                                    message.into_bytes(),
                                ),
                                RuntimeHttpClientEvent::Closed => {
                                    // A client connection disappearing is an abort: pending
                                    // request/response writes must be cancelled before close.
                                    close_transport = true;
                                    close_mode = HttpCloseMode::Abort;
                                    (
                                        0,
                                        HTTP_EVENT_CLOSED,
                                        0,
                                        0,
                                        Vec::new(),
                                        Vec::new(),
                                        Vec::new(),
                                    )
                                }
                            };
                            payloads.push(HttpEventPayload {
                                owner: Rc::clone(&owner),
                                kind,
                                flags,
                                id,
                                status,
                                version_minor: 0,
                                method: Vec::new(),
                                target: Vec::new(),
                                reason,
                                headers,
                                data,
                            });
                        }
                    }
                }
                let had_events = !payloads.is_empty();
                event_count = event_count.saturating_add(payloads.len());
                for payload in payloads {
                    let status = queue_http_event(realm, payload);
                    if status != 0 {
                        return Err(status);
                    }
                }
                if close_transport {
                    let actions = if in_context {
                        let Some(runtime) = active_runtime() else {
                            return Err(INVALID_ARGUMENT);
                        };
                        // Hyper may publish `Closed` in the same poll that queued its final
                        // response bytes. Drain those bytes before asking the transport to close
                        // so shutdown is ordered after the final HostOps write.
                        unsafe { &mut *runtime }.poll_http_connection_in_context(resource)
                    } else {
                        realm.runtime.borrow_mut().poll_http_connection(resource)
                    };
                    let Some(actions) = actions else {
                        return Err(RUNTIME_ERROR);
                    };
                    let status = apply_http_actions(realm, resource, actions, in_context);
                    if status != 0 {
                        return Err(status);
                    }
                    let actions = if in_context {
                        let Some(runtime) = active_runtime() else {
                            return Err(INVALID_ARGUMENT);
                        };
                        // SAFETY: this is the current realm's exclusively active runtime.
                        unsafe { &mut *runtime }.close_http_connection(resource, close_mode)
                    } else {
                        realm
                            .runtime
                            .borrow_mut()
                            .close_http_connection(resource, close_mode)
                    };
                    let Some(actions) = actions else {
                        return Err(RUNTIME_ERROR);
                    };
                    let status = apply_http_actions(realm, resource, actions, in_context);
                    if status != 0 {
                        return Err(status);
                    }
                }
                if !had_actions && !had_events {
                    break;
                }
                if !in_context {
                    // Applying actions and draining body events can wake the connection future
                    // again (Finish releases the next buffered HTTP/1 request). Advance that
                    // concrete progress before deciding the checkpoint is quiescent.
                    realm.runtime.borrow_mut().poll_http_tasks_after_wake();
                }
            }
        }
        Ok(event_count)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_realm_http_server_close_active(resource: u64, mode: i32) -> i32 {
        ffi(|| {
            let (Some(realm), Some(runtime)) = (active_realm(), active_runtime()) else {
                return INVALID_ARGUMENT;
            };
            let mode = match mode {
                HTTP_CLOSE_GRACEFUL => HttpCloseMode::Graceful,
                HTTP_CLOSE_ABORT => HttpCloseMode::Abort,
                _ => return INVALID_ARGUMENT,
            };
            let resource = kiln_host_abi::ResourceId::new(resource);
            // SAFETY: this private ABI call runs synchronously inside the active realm callback.
            let Some(actions) = (unsafe { &mut *runtime }).close_http_connection(resource, mode)
            else {
                return INVALID_ARGUMENT;
            };
            apply_http_actions(&realm, resource, actions, true)
        })
    }

    fn drive(realm: &NativeRealm, host: *mut c_void, thrown: *mut c_void) -> i32 {
        loop {
            // Neither lane checkpoints between items. A checkpoint drives the Tokio scheduler,
            // and doing that after every microtask made the per-request cost scale with how many
            // microtasks a request happens to enqueue rather than with the work it does. The
            // checkpoint below still runs before the loop can block, so nothing a callback wrote
            // waits on a socket: it is deferred to the end of the drain, not past it. Batching it
            // also lets one flush carry the writes of a whole turn.
            let next_tick = realm.work.borrow_mut().pop_next_tick();
            if let Some(work) = next_tick {
                let status = run_in_runtime(realm, || work.run(host, thrown));
                if status != 0 {
                    return status;
                }
                continue;
            }

            let microtask = realm.work.borrow_mut().pop_microtask();
            if let Some(work) = microtask {
                let status = run_in_runtime(realm, || work.run(host, thrown));
                if status != 0 {
                    return status;
                }
                continue;
            }

            // HTTP protocol futures can become runnable without a HostOps completion. A
            // checkpoint publishes their events into the same FIFO microtask lane; when it did,
            // loop immediately so callbacks run before any blocking host turn is selected.
            match checkpoint_http(realm, false) {
                Ok(events) if events != 0 => continue,
                Ok(_) => {}
                Err(status) => return status,
            }
            let turn = {
                let mut runtime = realm.runtime.borrow_mut();
                let mut work = realm.work.borrow_mut();
                runtime.next_referenced_turn(&mut work)
            };
            match turn {
                Some(RuntimeTurn::Completion(completion)) => {
                    let http_resource =
                        { realm.http_requests.borrow_mut().remove(&completion.request) };
                    if let Some(resource) = http_resource {
                        let Some(actions) = realm
                            .runtime
                            .borrow_mut()
                            .complete_http_connection(resource, completion)
                        else {
                            return RUNTIME_ERROR;
                        };
                        let status = apply_http_actions(realm, resource, actions, false);
                        if status != 0 {
                            return status;
                        }
                        if let Err(status) = checkpoint_http_after_wake(realm) {
                            return status;
                        }
                        continue;
                    }
                    let continuation = realm.continuations.borrow_mut().remove(&completion.request);
                    let Some(continuation) = continuation else {
                        continue;
                    };
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
                    let Some(work) =
                        NativeWork::new(pointer, Some(run_completion), Some(drop_completion))
                    else {
                        // SAFETY: `pointer` was produced by Box::into_raw and ownership has not
                        // been transferred because the callback pair was rejected.
                        unsafe { drop_completion(pointer) };
                        return RUNTIME_ERROR;
                    };
                    realm.work.borrow_mut().queue_microtask(work);
                }
                Some(RuntimeTurn::Failure(_)) => return RUNTIME_ERROR,
                Some(RuntimeTurn::Timer(WorkItem::Timer { work, .. })) => {
                    let status = run_in_runtime(realm, || work.run(host, thrown));
                    if status != 0 {
                        return status;
                    }
                    if let Err(status) = checkpoint_http(realm, false) {
                        return status;
                    }
                }
                Some(RuntimeTurn::Timer(WorkItem::Microtask(_))) => {
                    unreachable!("native drive drains microtasks before selecting a turn")
                }
                None => return 0,
            }
        }
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

    /// Writes the active Realm's deterministic clock in ECMAScript milliseconds.
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
            let Ok(work) = realm.work.try_borrow() else {
                return RUNTIME_ERROR;
            };
            let milliseconds = work.now().as_millis() as f64;
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
            let Ok(work) = realm.work.try_borrow() else {
                return RUNTIME_ERROR;
            };
            let nanoseconds = work.now().as_nanos() as i128;
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
            let Some(work) = NativeWork::new(work, run, drop) else {
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
            let Some(work) = NativeWork::new(work, run, drop) else {
                return INVALID_ARGUMENT;
            };
            queue.queue_next_tick(work);
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
            let request = unsafe { &mut *runtime }.start_host_op_in_context(operation, referenced);
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
            // SAFETY: active_runtime points at this active realm's exclusively-entered runtime.
            let runtime = unsafe { &mut *runtime };
            let actions = match role {
                HttpRole::Server => {
                    runtime.start_http1_connection_with_options(resource, allow_half_close)
                }
                HttpRole::Client => runtime.start_http1_client_connection(resource),
            };
            realm.http_connections.borrow_mut().insert(
                resource,
                NativeHttpConnection {
                    role,
                    events: owner,
                },
            );
            apply_http_actions(&realm, resource, actions, true)
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
            if unsafe { &mut *runtime }.send_http1_client_request(
                resource,
                RuntimeHttpClientRequest {
                    method: method.to_owned(),
                    target: target.to_owned(),
                    headers,
                },
                id,
                high_water_mark,
            ) {
                0
            } else {
                RUNTIME_ERROR
            }
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
            let disposition = if end {
                unsafe { &mut *runtime }.end_http1_client_request(
                    kiln_host_abi::ResourceId::new(resource),
                    id,
                    data.to_vec(),
                )
            } else {
                unsafe { &mut *runtime }.write_http1_client_request(
                    kiln_host_abi::ResourceId::new(resource),
                    id,
                    data.to_vec(),
                )
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
            let Some(headers) = (unsafe { parse_headers(response.headers, response.headers_len) })
            else {
                return INVALID_ARGUMENT;
            };
            let Ok(high_water_mark) = usize::try_from(high_water_mark) else {
                return INVALID_ARGUMENT;
            };
            // SAFETY: active_runtime is exclusively owned by the active realm callback.
            if unsafe { &mut *runtime }.start_http1_response(
                kiln_host_abi::ResourceId::new(resource),
                id,
                response.status,
                headers,
                high_water_mark,
            ) {
                0
            } else {
                RUNTIME_ERROR
            }
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
            // SAFETY: active_runtime is exclusively owned by this active realm callback.
            let disposition = if end {
                unsafe { &mut *runtime }.end_http1_response(
                    kiln_host_abi::ResourceId::new(resource),
                    id,
                    data.to_vec(),
                )
            } else {
                unsafe { &mut *runtime }.write_http1_response(
                    kiln_host_abi::ResourceId::new(resource),
                    id,
                    data.to_vec(),
                )
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
    mod tests {
        use super::*;
        use std::fs;
        use std::ptr;
        use std::sync::atomic::{AtomicU64, Ordering};

        static TEST_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        (realm, kiln_realm_symbol_for, realm_symbol_for) => {
            const _: unsafe extern "C" fn(*const u8, u64, *mut u64) -> i32 = kiln_realm_symbol_for;
        };
        (realm, kiln_realm_symbol_key_for, realm_symbol_key_for) => {
            const _: unsafe extern "C" fn(u64, *mut *const u8, *mut u64) -> i32 = kiln_realm_symbol_key_for;
        };
        (realm, kiln_realm_now, realm_now) => {
            const _: unsafe extern "C" fn(*mut f64) -> i32 = kiln_realm_now;
        };
        (realm, kiln_realm_now_nanoseconds, realm_now_nanoseconds) => {
            const _: unsafe extern "C" fn(*mut i64, *mut u64) -> i32 = kiln_realm_now_nanoseconds;
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

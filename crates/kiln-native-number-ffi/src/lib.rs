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

#[cfg(feature = "realm")]
mod http {
    use super::INVALID_ARGUMENT;
    use kiln_runtime::node_http::{
        Http1Decoder, Http1ResponseDecoder, HttpResponse, HttpResponseEvent,
    };
    use std::slice;

    #[repr(C)]
    pub struct RequestView {
        pub method: *mut u8,
        pub method_capacity: usize,
        pub method_len: usize,
        pub target: *mut u8,
        pub target_capacity: usize,
        pub target_len: usize,
        pub version_minor: u8,
        pub keep_alive: u8,
    }

    #[repr(C)]
    pub struct ResponseEventView {
        pub kind: u8,
        pub reserved: u8,
        pub status: u16,
        pub reason: *mut u8,
        pub reason_capacity: usize,
        pub reason_len: usize,
        pub data: *mut u8,
        pub data_capacity: usize,
        pub data_len: usize,
        pub headers: *mut u8,
        pub headers_capacity: usize,
        pub headers_len: usize,
    }

    pub struct ResponseEvents(Vec<HttpResponseEvent>);

    fn checked_header_field_len(name_len: usize, value_len: usize) -> Option<usize> {
        name_len
            .checked_add(1)?
            .checked_add(value_len)?
            .checked_add(1)
    }

    fn checked_advance(offset: &mut usize, amount: usize) -> bool {
        let Some(next) = offset.checked_add(amount) else {
            return false;
        };
        *offset = next;
        true
    }

    /// Allocate an opaque incremental HTTP/1 decoder. The handle never exposes Rust layout.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_decoder_create(out: *mut *mut Http1Decoder) -> i32 {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: null was rejected and the caller promises a writable pointer slot.
        unsafe { out.write(Box::into_raw(Box::new(Http1Decoder::default()))) };
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_decoder_free(decoder: *mut Http1Decoder) {
        if !decoder.is_null() {
            // SAFETY: the private ABI requires a live handle returned by create, exactly once.
            drop(unsafe { Box::from_raw(decoder) });
        }
    }

    /// Push one transport chunk and borrow the first complete request for this callback only.
    ///
    /// Returns 0 for a request, 1 when more bytes are needed, and a negative value for malformed
    /// input. This initial native server slice intentionally accepts one request per connection;
    /// the decoder itself remains pipelining-safe for the later keep-alive adapter.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_decoder_push(
        decoder: *mut Http1Decoder,
        bytes: *const u8,
        bytes_len: usize,
        out: *mut RequestView,
    ) -> i32 {
        if decoder.is_null() || out.is_null() || (bytes.is_null() && bytes_len != 0) {
            return INVALID_ARGUMENT;
        }
        let chunk = if bytes_len == 0 {
            &[]
        } else {
            // SAFETY: the caller supplies a readable span for this call.
            unsafe { slice::from_raw_parts(bytes, bytes_len) }
        };
        // SAFETY: the handle contract guarantees exclusive access during this call.
        let decoder = unsafe { &mut *decoder };
        let Ok(requests) = decoder.push(chunk) else {
            return -2;
        };
        let Some(request) = requests.first() else {
            return 1;
        };
        // SAFETY: `out` is writable by contract and is initialized by the caller with its owned
        // output spans. No pointer into a temporary Rust request crosses the boundary.
        let view = unsafe { &mut *out };
        if request.method.len() > view.method_capacity
            || request.target.len() > view.target_capacity
        {
            return -3;
        }
        if (!view.method.is_null() || request.method.is_empty())
            && (!view.target.is_null() || request.target.is_empty())
        {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    request.method.as_ptr(),
                    view.method,
                    request.method.len(),
                );
                std::ptr::copy_nonoverlapping(
                    request.target.as_ptr(),
                    view.target,
                    request.target.len(),
                );
            }
        } else {
            return INVALID_ARGUMENT;
        }
        view.method_len = request.method.len();
        view.target_len = request.target.len();
        view.version_minor = u8::from(matches!(
            request.version,
            kiln_runtime::node_http::HttpVersion::Http11
        ));
        view.keep_alive = u8::from(request.keep_alive);
        0
    }

    /// Encode one complete response through the shared Rust HTTP implementation.
    ///
    /// A null output buffer queries the required length; a sufficiently large buffer receives the
    /// bytes. Header pairs are passed as parallel borrowed spans to keep the ABI layout explicit.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_encode(
        status: u16,
        header_names: *const *const u8,
        header_name_lens: *const usize,
        header_values: *const *const u8,
        header_value_lens: *const usize,
        header_count: usize,
        body: *const u8,
        body_len: usize,
        keep_alive: u8,
        buffer: *mut u8,
        capacity: usize,
    ) -> isize {
        if (body.is_null() && body_len != 0)
            || (header_count != 0
                && (header_names.is_null()
                    || header_name_lens.is_null()
                    || header_values.is_null()
                    || header_value_lens.is_null()))
        {
            return INVALID_ARGUMENT as isize;
        }
        let names = unsafe { slice::from_raw_parts(header_names, header_count) };
        let name_lens = unsafe { slice::from_raw_parts(header_name_lens, header_count) };
        let values = unsafe { slice::from_raw_parts(header_values, header_count) };
        let value_lens = unsafe { slice::from_raw_parts(header_value_lens, header_count) };
        let mut headers = Vec::with_capacity(header_count);
        for index in 0..header_count {
            let Some((&name, &name_len)) = names.get(index).zip(name_lens.get(index)) else {
                return INVALID_ARGUMENT as isize;
            };
            let Some((&value, &value_len)) = values.get(index).zip(value_lens.get(index)) else {
                return INVALID_ARGUMENT as isize;
            };
            if (name.is_null() && name_len != 0) || (value.is_null() && value_len != 0) {
                return INVALID_ARGUMENT as isize;
            }
            let name = unsafe { slice::from_raw_parts(name, name_len) };
            let value = unsafe { slice::from_raw_parts(value, value_len) };
            let (Ok(name), Ok(value)) = (str::from_utf8(name), str::from_utf8(value)) else {
                return -2;
            };
            headers.push((name.to_owned(), value.to_owned()));
        }
        let body = unsafe { slice::from_raw_parts(body, body_len) }.to_vec();
        let encoded = HttpResponse {
            status,
            reason: None,
            headers,
            body,
        }
        .encode(keep_alive != 0);
        let Ok(length) = isize::try_from(encoded.len()) else {
            return INVALID_ARGUMENT as isize;
        };
        if buffer.is_null() {
            return length;
        }
        if capacity < encoded.len() {
            return INVALID_ARGUMENT as isize;
        }
        // SAFETY: the caller provides writable storage for at least the encoded length.
        unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), buffer, encoded.len()) };
        length
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_decoder_create(
        out: *mut *mut Http1ResponseDecoder,
    ) -> i32 {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        unsafe { out.write(Box::into_raw(Box::new(Http1ResponseDecoder::default()))) };
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_decoder_free(decoder: *mut Http1ResponseDecoder) {
        if !decoder.is_null() {
            drop(unsafe { Box::from_raw(decoder) });
        }
    }

    /// Decode one transport chunk into an ordered batch of head, data, and end events.
    ///
    /// The adapter drains every returned event before pushing another chunk. Event bytes are
    /// copied into caller-owned buffers, so no Rust allocation or layout crosses the ABI.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_decoder_push(
        decoder: *mut Http1ResponseDecoder,
        bytes: *const u8,
        bytes_len: usize,
        out: *mut *mut ResponseEvents,
    ) -> i32 {
        if decoder.is_null() || out.is_null() || (bytes.is_null() && bytes_len != 0) {
            return INVALID_ARGUMENT;
        }
        let chunk = if bytes_len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(bytes, bytes_len) }
        };
        let Ok(events) = (unsafe { &mut *decoder }).push_events(chunk) else {
            return -2;
        };
        unsafe { out.write(Box::into_raw(Box::new(ResponseEvents(events)))) };
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_events_len(
        events: *const ResponseEvents,
    ) -> usize {
        if events.is_null() {
            return 0;
        }
        unsafe { &*events }.0.len()
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_events_get(
        events: *const ResponseEvents,
        index: usize,
        out: *mut ResponseEventView,
    ) -> i32 {
        if events.is_null() || out.is_null() {
            return INVALID_ARGUMENT;
        }
        let Some(event) = (unsafe { &*events }).0.get(index) else {
            return INVALID_ARGUMENT;
        };
        let view = unsafe { &mut *out };
        view.kind = match event {
            HttpResponseEvent::Head(_) => 1,
            HttpResponseEvent::Data(_) => 2,
            HttpResponseEvent::End => 3,
        };
        match event {
            HttpResponseEvent::Head(head) => {
                let Some(header_len) =
                    head.headers
                        .iter()
                        .try_fold(0usize, |total, (name, value)| {
                            total.checked_add(checked_header_field_len(name.len(), value.len())?)
                        })
                else {
                    return INVALID_ARGUMENT;
                };
                view.status = head.status;
                view.reason_len = head.reason.len();
                view.headers_len = header_len;
                if head.reason.len() > view.reason_capacity
                    || header_len > view.headers_capacity
                    || (view.reason.is_null() && !head.reason.is_empty())
                    || (view.headers.is_null() && header_len != 0)
                {
                    return -3;
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        head.reason.as_ptr(),
                        view.reason,
                        head.reason.len(),
                    )
                };
                let mut at = 0;
                for (name, value) in &head.headers {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            name.as_ptr(),
                            view.headers.add(at),
                            name.len(),
                        );
                    }
                    if !checked_advance(&mut at, name.len()) {
                        return INVALID_ARGUMENT;
                    }
                    unsafe { view.headers.add(at).write(0) };
                    if !checked_advance(&mut at, 1) {
                        return INVALID_ARGUMENT;
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            value.as_ptr(),
                            view.headers.add(at),
                            value.len(),
                        );
                    }
                    if !checked_advance(&mut at, value.len()) {
                        return INVALID_ARGUMENT;
                    }
                    unsafe { view.headers.add(at).write(0) };
                    if !checked_advance(&mut at, 1) {
                        return INVALID_ARGUMENT;
                    }
                }
            }
            HttpResponseEvent::Data(data) => {
                view.data_len = data.len();
                if data.len() > view.data_capacity || (view.data.is_null() && !data.is_empty()) {
                    return -3;
                }
                unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), view.data, data.len()) };
            }
            HttpResponseEvent::End => {}
        };
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_http1_response_events_free(events: *mut ResponseEvents) {
        if !events.is_null() {
            drop(unsafe { Box::from_raw(events) });
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{checked_advance, checked_header_field_len};

        #[test]
        fn response_view_lengths_fail_closed_at_usize_boundary() {
            assert_eq!(checked_header_field_len(0, 0), Some(2));
            assert_eq!(checked_header_field_len(usize::MAX, 0), None);
            assert_eq!(checked_header_field_len(0, usize::MAX), None);

            let mut offset = usize::MAX - 1;
            assert!(checked_advance(&mut offset, 1));
            assert_eq!(offset, usize::MAX);
            assert!(!checked_advance(&mut offset, 1));
            assert_eq!(offset, usize::MAX);
        }
    }
}

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
        Realm, RealmRuntime, RuntimeTurn, TimerId, WorkItem, duration_from_milliseconds,
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
        cancelled: Rc<RefCell<HashSet<RequestId>>>,
        queued: Rc<RefCell<HashSet<RequestId>>>,
        entered: Cell<bool>,
    }

    impl NativeRealm {
        fn new() -> Result<Self, kiln_runtime::RuntimeBuildError> {
            Ok(Self {
                runtime: RefCell::new(RealmRuntime::new()?),
                work: RefCell::new(Realm::new()),
                continuations: RefCell::new(HashMap::new()),
                cancelled: Rc::new(RefCell::new(HashSet::new())),
                queued: Rc::new(RefCell::new(HashSet::new())),
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
        realm.runtime.borrow_mut().enter(work)
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

    fn drive(realm: &NativeRealm, host: *mut c_void, thrown: *mut c_void) -> i32 {
        loop {
            let microtask = realm.work.borrow_mut().pop_microtask();
            if let Some(work) = microtask {
                let status = run_in_runtime(realm, || work.run(host, thrown));
                if status != 0 {
                    return status;
                }
                continue;
            }

            let turn = {
                let has_pending = realm.runtime.borrow().has_pending_host_work();
                let mut runtime = realm.runtime.borrow_mut();
                let mut work = realm.work.borrow_mut();
                if has_pending {
                    runtime.next_turn(&mut work)
                } else {
                    runtime.next_timer(&mut work).map(RuntimeTurn::Timer)
                }
            };
            match turn {
                Some(RuntimeTurn::Completion(completion)) => {
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
                Some(RuntimeTurn::Timer(WorkItem::Timer { work, .. })) => {
                    let status = run_in_runtime(realm, || work.run(host, thrown));
                    if status != 0 {
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
            realm
                .runtime
                .borrow_mut()
                .enter(|| unsafe { entry(context) })
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
            flags: 0,
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
                if (*work).clear != 0 {
                    return kiln_realm_clear_timer((*work).clear);
                }
            }
            0
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
            let microtask = recorded_work(1, order, drops);
            // SAFETY: callbacks match the transferred allocation.
            let mut status = unsafe {
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
            assert_eq!(observed.0, [1, 2, 3, 4]);
            assert_eq!(kiln_realm_destroy(handle), 0);
            assert_eq!(observed.1, 4);
        }
    }
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

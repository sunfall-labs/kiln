//! HTTP server connections as Hyper tasks over host transports (ADR 0046).
//!
//! A connection is one Hyper future over a transport the host handed over. Its service calls the
//! realm's adapter directly, on the realm thread, inside the task's poll; the adapter answers
//! through a [`ResponseSlot`] the service future awaits. Nothing is spawned per read, queued per
//! event or re-sequenced per completion, and nothing on the request path allocates beyond what
//! Hyper parsed: the event borrows Hyper's method, target and header bytes, flattened into a
//! scratch buffer the connection keeps. Body events that would otherwise arrive inside the
//! language's own call (`finish` from within `res.end()`) are deferred to the next safe point so
//! the language never observes a callback from inside another.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::hash::{BuildHasherDefault, Hasher};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Body as _, Frame, Incoming};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::service::{Service, service_fn};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto;
use kiln_host_abi::ResourceId;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::Notify;

use crate::HttpCloseMode;
use crate::http_body::{BodyEvent, BodySink, ResponseBody, ResponseBodyDriver};
use crate::node_stream::WriteDisposition;

fn configured_http3() -> bool {
    static FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FROM_ENV.get_or_init(|| {
        let env_set =
            std::env::var_os("KILN_HTTP3").is_some_and(|value| !value.is_empty() && value != "0");
        let arg_set = std::env::args_os().any(|arg| arg == "--http3");
        env_set || arg_set
    })
}

#[derive(Clone, Copy, Debug)]
struct LocalExecutor;

impl<Fut> hyper::rt::Executor<Fut> for LocalExecutor
where
    Fut: Future + 'static,
    Fut::Output: 'static,
{
    fn execute(&self, fut: Fut) {
        tokio::task::spawn_local(fut);
    }
}

/// `KILN_HTTP_TRACE=1` prints the engine's decisions to stderr.
pub fn trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("KILN_HTTP_TRACE").is_some_and(|value| !value.is_empty() && value != "0")
    })
}

macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::http_engine::trace_enabled() {
            eprintln!("[kiln-http] {}", format_args!($($arg)*));
        }
    };
}

pub(crate) use trace;

/// An owned byte transport a host handed to the HTTP engine: the live socket, or a scripted
/// duplex that replays recorded bytes and captures what is written.
pub struct HostTransport(pub Box<dyn HostIo>);

/// What a transport must be for Hyper: nothing more than Tokio's I/O traits.
pub trait HostIo: AsyncRead + AsyncWrite + Unpin {}

impl<T: AsyncRead + AsyncWrite + Unpin> HostIo for T {}

const HTTP2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Replays only the bounded protocol-sniff bytes before delegating to the original transport.
/// This mirrors hyper-util's version detector without introducing HTTP framing logic, while
/// allowing the ordinary Hyper path to retain the bytes it already consumed.
struct PrefixIo<T> {
    inner: T,
    prefix: Vec<u8>,
    offset: usize,
}

impl<T> PrefixIo<T> {
    fn new(inner: T, prefix: Vec<u8>) -> Self {
        Self {
            inner,
            prefix,
            offset: 0,
        }
    }

    fn into_parts(self) -> (T, Vec<u8>) {
        (self.inner, self.prefix[self.offset..].to_vec())
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrefixIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.offset < self.prefix.len() {
            let length = (self.prefix.len() - self.offset).min(buffer.remaining());
            let end = self.offset + length;
            buffer.put_slice(&self.prefix[self.offset..end]);
            self.offset = end;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PrefixIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, bytes)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn detect_server_protocol(
    mut io: Box<dyn HostIo>,
) -> std::io::Result<(bool, PrefixIo<Box<dyn HostIo>>)> {
    let mut prefix = Vec::with_capacity(HTTP2_PREFACE.len());
    let mut is_http2 = true;
    while prefix.len() < HTTP2_PREFACE.len() {
        let before = prefix.len();
        let mut bytes = [0u8; 24];
        let capacity = HTTP2_PREFACE.len() - before;
        let length = io.read(&mut bytes[..capacity]).await?;
        if length == 0 {
            is_http2 = false;
            break;
        }
        prefix.extend_from_slice(&bytes[..length]);
        if prefix[before..] != HTTP2_PREFACE[before..before + length] {
            is_http2 = false;
            break;
        }
    }
    Ok((is_http2, PrefixIo::new(io, prefix)))
}

/// A byte stream Hyper released after an HTTP/1 protocol upgrade. `head` contains bytes Hyper
/// had already read beyond the HTTP request/response header; callers deliver it exactly once
/// before starting ordinary socket reads.
pub struct UpgradeTransport {
    pub transport: HostTransport,
    pub head: Vec<u8>,
    /// Bytes written after the 101 response head while Hyper still owned the stream. The host
    /// installs these as a prefix so they precede every later raw-socket write.
    pub pending_write: Vec<u8>,
    /// Completes when the host has written `pending_write`, allowing the engine to publish
    /// `Drain` only after those bytes have left the user-space queue.
    pub write_completion: Arc<UpgradeWriteCompletion>,
}

#[derive(Clone)]
struct UpgradeWriteFailure {
    kind: std::io::ErrorKind,
    message: String,
}

/// Completion for the first raw-protocol write queued before Hyper released the stream. This
/// state is shared with the host writer; readers never wait on it, preserving full-duplex I/O.
pub struct UpgradeWriteCompletion {
    result: Mutex<Option<Result<(), UpgradeWriteFailure>>>,
    notify: Notify,
}

impl UpgradeWriteCompletion {
    fn pending() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            notify: Notify::new(),
        })
    }

    pub(crate) fn complete(&self, result: std::io::Result<()>) {
        let result = result.map_err(|error| UpgradeWriteFailure {
            kind: error.kind(),
            message: error.to_string(),
        });
        if let Ok(mut slot) = self.result.lock()
            && slot.is_none()
        {
            *slot = Some(result);
            self.notify.notify_waiters();
        }
    }

    pub(crate) async fn wait(&self) -> std::io::Result<()> {
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.result.lock().ok().and_then(|slot| slot.clone()) {
                return result
                    .map_err(|failure| std::io::Error::new(failure.kind, failure.message));
            }
            notified.await;
        }
    }

    pub(crate) fn completed() -> Arc<Self> {
        let completion = Self::pending();
        completion.complete(Ok(()));
        completion
    }
}

impl std::fmt::Debug for HostTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostTransport")
    }
}

/// One protocol event delivered to the language adapter, for either role of a connection.
/// Every span is borrowed for the call.
#[derive(Debug, Clone, Copy)]
pub struct HttpEvent<'a> {
    pub kind: HttpEventKind,
    /// The request the event belongs to; zero for a connection-level event.
    pub id: u64,
    /// `RequestHead`: the peer asked to keep the connection open. `ResponseEnd`: the connection
    /// can carry another request, which is the Agent's reuse decision.
    pub keep_alive: bool,
    pub version_minor: u8,
    /// `ResponseHead` only.
    pub status: u16,
    pub method: &'a [u8],
    pub target: &'a [u8],
    /// `ResponseHead` only: the status text.
    pub reason: &'a [u8],
    /// The header block. On a connection that takes requests whole (a fetch-style `serve`
    /// server) it is the header-list wire form, `\nname:value` per field ([`flatten_header_wire`]);
    /// otherwise `name NUL value NUL` per header ([`flatten_header_map`]).
    pub headers: &'a [u8],
    pub data: &'a [u8],
    pub message: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpEventKind {
    RequestHead,
    /// An HTTP/1 request with an Upgrade header. The language receives it before sending the
    /// 101 response so it can write the protocol handshake through `write_upgrade`. `data` is
    /// empty: Hyper exposes bytes it read beyond the request head only after the response
    /// transfers the connection, and those bytes are then delivered once as the socket's first
    /// `data` event on `UpgradeReady`.
    UpgradeRequest,
    /// The HTTP parser has transferred an accepted upgrade transport back to the host. `data`
    /// contains bytes Hyper had already read beyond the request head.
    UpgradeReady,
    /// An HTTP client received a 101 and owns the raw upgraded stream after this event.
    UpgradeResponse,
    RequestData,
    RequestEnd,
    /// The body the language is writing (a response, or a client request) took a queued chunk.
    Drain,
    /// Every byte of that body is in Hyper's hands.
    Finish,
    ResponseHead,
    ResponseData,
    ResponseEnd,
    Error,
    Closed,
}

impl HttpEvent<'static> {
    pub(crate) fn bare(kind: HttpEventKind, id: u64) -> Self {
        Self {
            kind,
            id,
            keep_alive: false,
            version_minor: 0,
            status: 0,
            method: &[],
            target: &[],
            reason: &[],
            headers: &[],
            data: &[],
            message: &[],
        }
    }
}

/// The realm's side of a connection. `event` runs the language; `checkpoint` drains what the
/// language queued (next-ticks, microtasks, the rejection check) before the engine continues.
/// A non-zero return is a realm failure and ends the connection.
pub trait HttpDispatch {
    /// Called when the connection task reaches its first poll. This is an admission
    /// notification, not a request-ready notification.
    fn started(&self) {}
    fn event(&self, event: &HttpEvent<'_>) -> i32;
    fn checkpoint(&self) -> i32;
    /// Whether the language wants each request whole: the engine collects the request body
    /// before dispatching, delivers it as the `RequestHead` event's `data`, and dispatches no
    /// `RequestData`/`RequestEnd` for it. This is the shape a fetch-style handler (`serve`)
    /// consumes -- one crossing per request instead of three -- and it is fixed for the life of
    /// the connection. Node's `IncomingMessage` streams, so the default is false.
    fn whole_requests(&self) -> bool {
        false
    }
}

/// What both engines keep per connection: the realm's dispatch, and the body events produced
/// inside a language call that must reach the language only after it returns.
pub(crate) struct Dispatcher {
    pub(crate) resource: ResourceId,
    dispatch: Rc<dyn HttpDispatch>,
    /// Body events (`drain`, `finish`) produced inside a language call, delivered after it.
    deferred: RefCell<Vec<(u64, BodyEvent)>>,
    pub(crate) deferred_wake: Notify,
    failed: Cell<bool>,
    /// [`HttpDispatch::whole_requests`], read once.
    whole_requests: bool,
}

impl Dispatcher {
    pub(crate) fn new(resource: ResourceId, dispatch: Rc<dyn HttpDispatch>) -> Self {
        let whole_requests = dispatch.whole_requests();
        Self {
            resource,
            dispatch,
            deferred: RefCell::new(Vec::new()),
            deferred_wake: Notify::new(),
            failed: Cell::new(false),
            whole_requests,
        }
    }

    pub(crate) fn whole_requests(&self) -> bool {
        self.whole_requests
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.get()
    }

    pub(crate) fn started(&self) {
        self.dispatch.started();
    }

    pub(crate) fn set_failed(&self, failed: bool) {
        self.failed.set(failed);
    }

    /// Run the language for one event and then its checkpoint. False once the realm failed.
    pub(crate) fn dispatch(&self, event: &HttpEvent<'_>) -> bool {
        if self.failed.get() {
            return false;
        }
        trace!(
            "{:?} dispatch {:?} id={}",
            self.resource, event.kind, event.id
        );
        let status = self.dispatch.event(event);
        if status != 0 {
            self.failed.set(true);
            return false;
        }
        let status = self.dispatch.checkpoint();
        if status != 0 {
            self.failed.set(true);
            return false;
        }
        true
    }

    pub(crate) fn defer(&self, id: u64, event: BodyEvent) {
        self.deferred.borrow_mut().push((id, event));
        self.deferred_wake.notify_one();
    }

    pub(crate) fn flush_deferred(&self) -> bool {
        loop {
            let pending = std::mem::take(&mut *self.deferred.borrow_mut());
            if pending.is_empty() {
                return true;
            }
            for (id, event) in pending {
                let kind = match event {
                    BodyEvent::Drain => HttpEventKind::Drain,
                    BodyEvent::Finish => HttpEventKind::Finish,
                };
                if !self.dispatch(&HttpEvent::bare(kind, id)) {
                    return false;
                }
            }
        }
    }

    /// Deliver deferred body events as they are produced, for as long as the realm is healthy.
    /// Resolves only once a dispatch fails, so a `select!` arm on it runs the pump for the life
    /// of the connection.
    pub(crate) async fn pump(&self) {
        loop {
            self.deferred_wake.notified().await;
            if !self.flush_deferred() {
                return;
            }
        }
    }

    /// A body driver whose `drain` and `finish` are deferred to this dispatcher's next safe
    /// point.
    pub(crate) fn body_driver(
        self: &Rc<Self>,
        id: u64,
        high_water_mark: usize,
    ) -> ResponseBodyDriver {
        let driver = ResponseBodyDriver::new(high_water_mark);
        let sink = Rc::clone(self);
        driver.set_sink(BodySink::Direct(Rc::new(move |event| {
            sink.defer(id, event)
        })));
        driver
    }
}

/// The engine's maps are keyed by small integers (resource and request ids), so they hash by
/// one multiplication rather than SipHash, which showed up in the request profile.
#[derive(Default)]
pub(crate) struct IntHasher(u64);

impl Hasher for IntHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(u64::from(*byte));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0 ^ value).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<IntHasher>>;

/// The table of live server connections. Lives in the realm state; tasks reach it through
/// `Rc` so a finished connection can retire itself.
pub struct HttpServerEngine {
    connections: Rc<RefCell<HashMap<ResourceId, ServerConnection>>>,
    upgraded: Rc<RefCell<HashMap<ResourceId, UpgradeTransport>>>,
    http3: bool,
}

impl Default for HttpServerEngine {
    fn default() -> Self {
        Self::with_http3(configured_http3())
    }
}

impl HttpServerEngine {
    pub fn with_http3(http3: bool) -> Self {
        Self {
            connections: Rc::new(RefCell::new(HashMap::default())),
            upgraded: Rc::new(RefCell::new(HashMap::default())),
            http3,
        }
    }

    /// Queue bytes written by a Node `upgrade` listener while Hyper still owns the connection.
    /// The first complete HTTP response head commits the 101; following bytes are sent over the
    /// transport after Hyper hands it back.
    pub fn write_upgrade(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        let shared = self.connections.borrow().get(&resource)?.shared.clone();
        let writes = shared.upgrade_writes.borrow().get(&id)?.clone();
        let Some(completed) = writes.append(data) else {
            return None;
        };
        if let Some((status, headers, trailing)) = completed {
            let mut flattened = Vec::new();
            flatten_header_map(&headers, &mut flattened);
            if !self.start_response(resource, id, status, &flattened, 16 * 1024)
                || self
                    .end_response_bytes(resource, id, Bytes::new())
                    .is_none()
            {
                return None;
            }
            writes.add_trailing(&trailing);
        }
        let ready = writes.pending_bytes() < UpgradeWrite::HIGH_WATER_MARK;
        if !ready {
            writes.drain_needed.set(true);
        }
        Some(WriteDisposition {
            ready,
            submit_now: true,
        })
    }

    /// Take the raw stream after an upgrade was accepted. This removes the protocol task's
    /// ownership; the host must adopt it under the same resource before publishing `UpgradeReady`.
    pub fn take_upgrade(&mut self, resource: ResourceId) -> Option<UpgradeTransport> {
        self.upgraded.borrow_mut().remove(&resource)
    }
}

struct ServerConnection {
    shared: Rc<ConnectionShared>,
    task: tokio::task::JoinHandle<()>,
}

pub(crate) struct ConnectionShared {
    resource: ResourceId,
    dispatcher: Rc<Dispatcher>,
    requests: RefCell<HashMap<u64, Rc<ResponseSlot>>>,
    /// The flattened request headers of the request being dispatched, reused per request.
    header_scratch: RefCell<Vec<u8>>,
    /// A whole request's URL (see [`request_url`]), reused per request.
    url_scratch: RefCell<Vec<u8>>,
    shutdown: Notify,
    closing: Cell<bool>,
    /// An abort has been requested and its `Closed` has not been published yet. Counted as
    /// work in flight so a drive does not return before the language hears it.
    abort_pending: Cell<bool>,
    http3: bool,
    upgrade_accepted: Cell<bool>,
    upgrade_request_id: Cell<u64>,
    upgrade_writes: RefCell<HashMap<u64, Rc<UpgradeWrite>>>,
}

struct UpgradeWrite {
    raw_head: RefCell<Vec<u8>>,
    trailing: RefCell<Vec<u8>>,
    parsed: Cell<bool>,
    drain_needed: Cell<bool>,
    completion: Arc<UpgradeWriteCompletion>,
}

impl UpgradeWrite {
    const MAX_BUFFERED_BYTES: usize = 1024 * 1024;
    const HIGH_WATER_MARK: usize = 16 * 1024;

    fn new() -> Self {
        Self {
            raw_head: RefCell::new(Vec::with_capacity(1024)),
            trailing: RefCell::new(Vec::new()),
            parsed: Cell::new(false),
            drain_needed: Cell::new(false),
            completion: UpgradeWriteCompletion::pending(),
        }
    }

    fn append(&self, data: &[u8]) -> Option<Option<(u16, HeaderMap, Vec<u8>)>> {
        if self.pending_bytes().checked_add(data.len())? > Self::MAX_BUFFERED_BYTES {
            return None;
        }
        if self.parsed.get() {
            self.trailing.borrow_mut().extend_from_slice(data);
            return Some(None);
        }
        let mut raw = self.raw_head.borrow_mut();
        raw.extend_from_slice(data);
        let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
            return (raw.len() <= 64 * 1024).then_some(None);
        };
        let end = end + 4;
        if end > 64 * 1024 {
            return None;
        }
        let (status, headers) = parse_upgrade_response_head(&raw[..end])?;
        let trailing = raw[end..].to_vec();
        raw.clear();
        self.parsed.set(true);
        Some(Some((status, headers, trailing)))
    }

    fn add_trailing(&self, data: &[u8]) {
        self.trailing.borrow_mut().extend_from_slice(data);
    }

    fn take_trailing(&self) -> Vec<u8> {
        std::mem::take(&mut *self.trailing.borrow_mut())
    }

    fn pending_bytes(&self) -> usize {
        self.raw_head.borrow().len() + self.trailing.borrow().len()
    }
}

impl ConnectionShared {
    fn dispatch(&self, event: &HttpEvent<'_>) -> bool {
        self.dispatcher.dispatch(event)
    }

    fn defer(&self, id: u64, event: BodyEvent) {
        if event == BodyEvent::Drain
            && let Some(slot) = self.requests.borrow().get(&id)
        {
            slot.notify_drain();
        }
        self.dispatcher.defer(id, event);
    }

    fn flush_deferred(&self) -> bool {
        self.dispatcher.flush_deferred()
    }
}

/// The body Hyper sends: nothing, one frame the language handed over whole, or a stream the
/// language is still writing.
pub(crate) enum EngineBody {
    Empty,
    Complete(Option<Bytes>),
    Streaming(ResponseBody),
}

impl hyper::body::Body for EngineBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.get_mut() {
            Self::Empty => Poll::Ready(None),
            Self::Complete(bytes) => Poll::Ready(bytes.take().map(|bytes| Ok(Frame::data(bytes)))),
            Self::Streaming(body) => Pin::new(body).poll_frame(context),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Complete(bytes) => bytes.is_none(),
            Self::Streaming(_) => false,
        }
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self {
            Self::Empty => hyper::body::SizeHint::with_exact(0),
            Self::Complete(bytes) => {
                hyper::body::SizeHint::with_exact(bytes.as_ref().map_or(0, |b| b.len() as u64))
            }
            Self::Streaming(_) => hyper::body::SizeHint::default(),
        }
    }
}

/// A response as the language produced it, ready for Hyper.
struct EngineResponse {
    status: u16,
    headers: HeaderMap,
    body: EngineBody,
}

/// Where a response accumulates until the service future can hand it to Hyper.
pub struct ResponseSlot {
    inner: RefCell<SlotInner>,
    piped_request: Cell<bool>,
    drain_notify: Notify,
}

enum ResponseBodyState {
    Complete(Bytes),
    Streaming {
        driver: ResponseBodyDriver,
        ended: bool,
    },
}

enum SlotState {
    /// No response has been started yet.
    Empty,
    /// Head provided by `start`, waiting for `write` or `end` to determine framing.
    Parked {
        status: u16,
        headers: HeaderMap,
        high_water_mark: usize,
    },
    /// Head is committed, waiting for Hyper's `SlotReady` future to take it.
    Ready {
        status: u16,
        headers: HeaderMap,
        body: ResponseBodyState,
        bodyless: bool,
    },
    /// Hyper took the response head; body stream is open for subsequent writes.
    Streaming {
        driver: ResponseBodyDriver,
        bodyless: bool,
    },
    /// Response has ended; no further writes or ends accepted.
    Finished,
}

struct SlotInner {
    head_request: bool,
    state: SlotState,
    waker: Option<Waker>,
}

impl ResponseSlot {
    fn new(head_request: bool) -> Self {
        Self {
            inner: RefCell::new(SlotInner {
                head_request,
                state: SlotState::Empty,
                waker: None,
            }),
            piped_request: Cell::new(false),
            drain_notify: Notify::new(),
        }
    }

    pub fn set_piped_request(&self, piped: bool) {
        self.piped_request.set(piped);
    }

    pub fn is_piped_request(&self) -> bool {
        self.piped_request.get()
    }

    pub fn notify_drain(&self) {
        self.drain_notify.notify_waiters();
    }

    pub async fn wait_for_drain(&self) {
        self.drain_notify.notified().await;
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.inner.borrow().state, SlotState::Empty)
    }

    pub(crate) fn ensure_streaming_ready(&self, shared: &Rc<ConnectionShared>, id: u64) {
        let mut inner = self.inner.borrow_mut();
        let head_request = inner.head_request;
        match &mut inner.state {
            SlotState::Empty => {
                let status = 200;
                let mut headers = HeaderMap::new();
                headers.insert(
                    hyper::header::TRANSFER_ENCODING,
                    HeaderValue::from_static("chunked"),
                );
                let high_water_mark = crate::node_stream::DEFAULT_HIGH_WATER_MARK;
                let carries_body = status_allows_body(status) && !head_request;
                let bodyless = !carries_body;
                let driver = body_driver(shared, id, high_water_mark);
                inner.state = SlotState::Ready {
                    status,
                    headers,
                    body: ResponseBodyState::Streaming {
                        driver,
                        ended: false,
                    },
                    bodyless,
                };
                if let Some(waker) = inner.waker.take() {
                    waker.wake();
                }
            }
            SlotState::Parked {
                status,
                headers,
                high_water_mark,
            } => {
                let status = *status;
                let mut headers = std::mem::take(headers);
                let high_water_mark = *high_water_mark;
                let carries_body = status_allows_body(status) && !head_request;
                let bodyless = !carries_body;
                if carries_body
                    && !headers.contains_key(hyper::header::CONTENT_LENGTH)
                    && !headers.contains_key(hyper::header::TRANSFER_ENCODING)
                {
                    headers.insert(
                        hyper::header::TRANSFER_ENCODING,
                        HeaderValue::from_static("chunked"),
                    );
                }
                let driver = body_driver(shared, id, high_water_mark);
                inner.state = SlotState::Ready {
                    status,
                    headers,
                    body: ResponseBodyState::Streaming {
                        driver,
                        ended: false,
                    },
                    bodyless,
                };
                if let Some(waker) = inner.waker.take() {
                    waker.wake();
                }
            }
            SlotState::Ready { .. } | SlotState::Streaming { .. } | SlotState::Finished => {}
        }
    }

    fn start(&self, status: u16, headers: HeaderMap, high_water_mark: usize) -> bool {
        let mut inner = self.inner.borrow_mut();
        if !matches!(inner.state, SlotState::Empty) {
            return false;
        }
        inner.state = SlotState::Parked {
            status,
            headers,
            high_water_mark: high_water_mark.max(1),
        };
        true
    }

    fn write_bytes(
        &self,
        shared: &Rc<ConnectionShared>,
        id: u64,
        data: Bytes,
    ) -> Option<WriteDisposition> {
        let mut inner = self.inner.borrow_mut();
        let head_request = inner.head_request;
        match &mut inner.state {
            SlotState::Parked {
                status,
                headers,
                high_water_mark,
            } => {
                let status = *status;
                let headers = std::mem::take(headers);
                let high_water_mark = *high_water_mark;
                let carries_body = status_allows_body(status) && !head_request;
                let bodyless = !carries_body;
                let mut driver = body_driver(shared, id, high_water_mark);
                let disposition = driver.write(data);
                inner.state = SlotState::Ready {
                    status,
                    headers,
                    body: ResponseBodyState::Streaming {
                        driver,
                        ended: false,
                    },
                    bodyless,
                };
                if let Some(waker) = inner.waker.take() {
                    waker.wake();
                }
                Some(disposition)
            }
            SlotState::Ready {
                body: ResponseBodyState::Streaming { driver, ended },
                ..
            } => {
                if *ended {
                    return None;
                }
                Some(driver.write(data))
            }
            SlotState::Streaming { driver, .. } => Some(driver.write(data)),
            SlotState::Empty
            | SlotState::Ready {
                body: ResponseBodyState::Complete(_),
                ..
            }
            | SlotState::Finished => None,
        }
    }

    fn write(
        &self,
        shared: &Rc<ConnectionShared>,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        self.write_bytes(shared, id, Bytes::copy_from_slice(data))
    }

    fn end(&self, shared: &Rc<ConnectionShared>, id: u64, data: Bytes) -> Option<WriteDisposition> {
        let mut inner = self.inner.borrow_mut();
        let head_request = inner.head_request;
        match &mut inner.state {
            SlotState::Parked {
                status, headers, ..
            } => {
                let status = *status;
                let mut headers = std::mem::take(headers);
                let carries_body = status_allows_body(status) && !head_request;
                let bodyless = !carries_body;
                if carries_body
                    && !headers.contains_key(hyper::header::CONTENT_LENGTH)
                    && !headers.contains_key(hyper::header::TRANSFER_ENCODING)
                {
                    headers.insert(hyper::header::CONTENT_LENGTH, HeaderValue::from(data.len()));
                }
                let bytes = if bodyless || data.is_empty() {
                    Bytes::new()
                } else {
                    data
                };
                inner.state = SlotState::Ready {
                    status,
                    headers,
                    body: ResponseBodyState::Complete(bytes),
                    bodyless,
                };
                if let Some(waker) = inner.waker.take() {
                    waker.wake();
                }
                shared.defer(id, BodyEvent::Finish);
                Some(WriteDisposition {
                    ready: true,
                    submit_now: false,
                })
            }
            SlotState::Ready {
                body: ResponseBodyState::Streaming { driver, ended },
                bodyless,
                ..
            } => {
                if *ended {
                    return None;
                }
                let disposition = driver.end(data);
                if *bodyless {
                    driver.finish_without_body();
                }
                *ended = true;
                Some(disposition)
            }
            SlotState::Streaming { driver, bodyless } => {
                let disposition = driver.end(data);
                if *bodyless {
                    driver.finish_without_body();
                }
                inner.state = SlotState::Finished;
                Some(disposition)
            }
            SlotState::Empty
            | SlotState::Ready {
                body: ResponseBodyState::Complete(_),
                ..
            }
            | SlotState::Finished => None,
        }
    }
}

fn body_driver(
    shared: &Rc<ConnectionShared>,
    id: u64,
    high_water_mark: usize,
) -> ResponseBodyDriver {
    shared.dispatcher.body_driver(id, high_water_mark)
}

pub(crate) fn status_allows_body(status: u16) -> bool {
    !matches!(status, 204 | 304) && !(100..200).contains(&status)
}

/// Parse `name NUL value NUL` pairs into a header map, refusing anything Hyper would refuse.
pub(crate) fn parse_flattened_headers(bytes: &[u8]) -> Option<HeaderMap> {
    let mut headers = HeaderMap::new();
    let mut fields = bytes.split(|byte| *byte == 0);
    while let Some(name) = fields.next() {
        if name.is_empty() {
            // The flattening ends every value with NUL, so the split yields one empty tail.
            break;
        }
        let value = fields.next()?;
        let name = HeaderName::from_bytes(name).ok()?;
        let value = HeaderValue::from_bytes(value).ok()?;
        headers.append(name, value);
    }
    Some(headers)
}

/// Resolves once the slot's head is committed, with the response Hyper will send.
struct SlotReady {
    slot: Rc<ResponseSlot>,
}

impl Future for SlotReady {
    type Output = EngineResponse;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.slot.inner.borrow_mut();
        match std::mem::replace(&mut inner.state, SlotState::Finished) {
            SlotState::Ready {
                status,
                headers,
                body,
                bodyless,
            } => {
                let response_body = match body {
                    ResponseBodyState::Complete(bytes) => {
                        inner.state = SlotState::Finished;
                        if bytes.is_empty() {
                            EngineBody::Empty
                        } else {
                            EngineBody::Complete(Some(bytes))
                        }
                    }
                    ResponseBodyState::Streaming { driver, ended } => {
                        let response_body = EngineBody::Streaming(driver.into_body());
                        if ended {
                            inner.state = SlotState::Finished;
                        } else {
                            inner.state = SlotState::Streaming { driver, bodyless };
                        }
                        response_body
                    }
                };
                Poll::Ready(EngineResponse {
                    status,
                    headers,
                    body: response_body,
                })
            }
            other => {
                inner.state = other;
                inner.waker = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl HttpServerEngine {
    /// Spawn the connection future for `resource` over `transport`, dispatching to `dispatch`.
    pub fn attach(
        &mut self,
        resource: ResourceId,
        transport: HostTransport,
        allow_half_close: bool,
        dispatch: Rc<dyn HttpDispatch>,
    ) -> bool {
        if self.connections.borrow().contains_key(&resource) {
            return false;
        }
        let shared = Rc::new(ConnectionShared {
            resource,
            dispatcher: Rc::new(Dispatcher::new(resource, dispatch)),
            requests: RefCell::new(HashMap::default()),
            header_scratch: RefCell::new(Vec::with_capacity(1024)),
            url_scratch: RefCell::new(Vec::with_capacity(128)),
            shutdown: Notify::new(),
            closing: Cell::new(false),
            abort_pending: Cell::new(false),
            http3: self.http3,
            upgrade_accepted: Cell::new(false),
            upgrade_request_id: Cell::new(0),
            upgrade_writes: RefCell::new(HashMap::default()),
        });
        trace!("{resource:?} attach half_close={allow_half_close}");
        let table = Rc::clone(&self.connections);
        let upgraded = Rc::clone(&self.upgraded);
        let task = tokio::task::spawn_local(serve(
            transport,
            allow_half_close,
            Rc::clone(&shared),
            table,
            upgraded,
        ));
        self.connections
            .borrow_mut()
            .insert(resource, ServerConnection { shared, task });
        true
    }

    pub fn is_attached(&self, resource: ResourceId) -> bool {
        self.connections.borrow().contains_key(&resource)
    }

    /// Record the head of a response. `headers` is the connection's header block: the wire form
    /// ([`parse_header_wire`]) on a connection that takes requests whole, `name NUL value NUL`
    /// otherwise. An invalid name or value refuses the head.
    pub fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: &[u8],
        high_water_mark: usize,
    ) -> bool {
        let (slot, whole) = {
            let table = self.connections.borrow();
            let Some(connection) = table.get(&resource) else {
                return false;
            };
            let Some(slot) = connection.shared.requests.borrow().get(&id).cloned() else {
                return false;
            };
            (slot, connection.shared.dispatcher.whole_requests())
        };
        let parsed = if whole {
            parse_header_wire(headers)
        } else {
            parse_flattened_headers(headers)
        };
        let Some(headers) = parsed else {
            return false;
        };
        slot.start(status, headers, high_water_mark)
    }

    /// A fetch-style connection's whole response in one call: the head, its headers in the
    /// header-list wire form, and the complete body. False when the request is gone or the head
    /// is refused.
    pub fn respond(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: &[u8],
        body: Bytes,
    ) -> bool {
        let Some((shared, slot)) = self.shared_and_slot(resource, id) else {
            return false;
        };
        let Some(headers) = parse_header_wire(headers) else {
            return false;
        };
        slot.start(status, headers, 16 * 1024) && slot.end(&shared, id, body).is_some()
    }

    pub fn write_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.write(&shared, id, data)
    }

    pub fn write_response_bytes(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Bytes,
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.write_bytes(&shared, id, data)
    }

    pub fn end_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.end(&shared, id, Bytes::copy_from_slice(data))
    }

    pub fn end_response_bytes(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Bytes,
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.end(&shared, id, data)
    }

    /// Send a static response using direct `Bytes` without extra intermediate copies.
    pub fn send_static_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: &[u8],
        body: Bytes,
    ) -> Option<WriteDisposition> {
        let headers = parse_flattened_headers(headers)?;
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        if !slot.start(status, headers, 1) {
            return None;
        }
        slot.end(&shared, id, body)
    }

    /// Send a static response using pre-built HeaderMap and direct `Bytes`.
    pub fn end_static_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: HeaderMap,
        body: Bytes,
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        if !slot.start(status, headers, 1) {
            return None;
        }
        slot.end(&shared, id, body)
    }

    /// Stop a connection. Graceful lets the response in flight finish; abort drops the
    /// transport and reports `Closed` on the next task turn, never inside the caller.
    pub fn close(&mut self, resource: ResourceId, mode: HttpCloseMode) -> bool {
        let shared = {
            let table = self.connections.borrow();
            let Some(connection) = table.get(&resource) else {
                return false;
            };
            if mode == HttpCloseMode::Abort {
                connection.task.abort();
            }
            Rc::clone(&connection.shared)
        };
        trace!("{resource:?} close {mode:?}");
        match mode {
            HttpCloseMode::Graceful => {
                shared.closing.set(true);
                shared.shutdown.notify_one();
            }
            HttpCloseMode::Abort => {
                // The abort takes effect at the task's next poll; until `Closed` has been
                // published the connection counts as in flight, and it is published from a
                // task, never from inside the caller's own call.
                shared.abort_pending.set(true);
                shared.dispatcher.set_failed(false);
                let table = Rc::clone(&self.connections);
                tokio::task::spawn_local(async move {
                    let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
                    shared.abort_pending.set(false);
                    table.borrow_mut().remove(&shared.resource);
                });
            }
        }
        true
    }

    pub fn open_connections(&self) -> usize {
        self.connections.borrow().len()
    }

    /// Abort every connection task and forget the table. Realm teardown; nothing is
    /// dispatched.
    pub fn shutdown(&mut self) {
        for (_, connection) in self.connections.borrow_mut().drain() {
            connection.task.abort();
        }
    }

    /// Requests whose response the language has not finished: what keeps a drive waiting
    /// when no host operation is pending.
    pub fn requests_in_flight(&self) -> usize {
        self.connections
            .borrow()
            .values()
            .map(|connection| {
                connection.shared.requests.borrow().len()
                    + usize::from(connection.shared.abort_pending.get())
            })
            .sum()
    }

    pub fn pipe_request(&mut self, resource: ResourceId, id: u64) -> bool {
        let Some((shared, slot)) = self.shared_and_slot(resource, id) else {
            return false;
        };
        slot.set_piped_request(true);
        slot.ensure_streaming_ready(&shared, id);
        true
    }

    pub fn response_pipe_target(
        &mut self,
        resource: ResourceId,
        id: u64,
    ) -> Option<Rc<RefCell<dyn crate::node_stream::PipeTarget>>> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        Some(Rc::new(RefCell::new(ResponseSlotPipeTarget::new(
            shared, slot, id,
        ))))
    }

    fn shared_and_slot(
        &self,
        resource: ResourceId,
        id: u64,
    ) -> Option<(Rc<ConnectionShared>, Rc<ResponseSlot>)> {
        let table = self.connections.borrow();
        let connection = table.get(&resource)?;
        let slot = connection.shared.requests.borrow().get(&id).cloned()?;
        Some((Rc::clone(&connection.shared), slot))
    }
}

#[derive(Clone)]
pub struct ResponseSlotPipeTarget {
    shared: Rc<ConnectionShared>,
    slot: Rc<ResponseSlot>,
    id: u64,
}

impl std::fmt::Debug for ResponseSlotPipeTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseSlotPipeTarget")
            .field("id", &self.id)
            .finish()
    }
}

impl ResponseSlotPipeTarget {
    pub(crate) fn new(shared: Rc<ConnectionShared>, slot: Rc<ResponseSlot>, id: u64) -> Self {
        slot.ensure_streaming_ready(&shared, id);
        Self { shared, slot, id }
    }
}

impl crate::node_stream::PipeTarget for ResponseSlotPipeTarget {
    fn write_chunk(&mut self, data: &[u8]) -> WriteDisposition {
        self.slot
            .write(&self.shared, self.id, data)
            .unwrap_or(WriteDisposition {
                ready: false,
                submit_now: false,
            })
    }

    fn end_pipe(&mut self) -> WriteDisposition {
        self.slot
            .end(&self.shared, self.id, Bytes::new())
            .unwrap_or(WriteDisposition {
                ready: true,
                submit_now: false,
            })
    }
}

fn server_service(
    shared: Rc<ConnectionShared>,
) -> impl Service<
    Request<Incoming>,
    Response = Response<EngineBody>,
    Error = std::convert::Infallible,
    Future = Pin<Box<dyn Future<Output = Result<Response<EngineBody>, std::convert::Infallible>>>>,
> + Unpin {
    let next_id = Cell::new(1_u64);
    service_fn(move |request: Request<Incoming>| {
        let id = next_id.get();
        next_id.set(id.wrapping_add(1).max(1));
        let shared = Rc::clone(&shared);
        Box::pin(
            async move { Ok::<_, std::convert::Infallible>(handle(shared, id, request).await) },
        )
            as Pin<Box<dyn Future<Output = Result<Response<EngineBody>, std::convert::Infallible>>>>
    })
}

async fn serve(
    transport: HostTransport,
    allow_half_close: bool,
    shared: Rc<ConnectionShared>,
    table: Rc<RefCell<HashMap<ResourceId, ServerConnection>>>,
    upgraded: Rc<RefCell<HashMap<ResourceId, UpgradeTransport>>>,
) {
    shared.dispatcher.started();
    let sniff = detect_server_protocol(transport.0);
    tokio::pin!(sniff);
    let mut pump_done = false;
    let (is_http2, io) = loop {
        tokio::select! {
            result = &mut sniff => match result {
                Ok(protocol) => break protocol,
                Err(error) => {
                    let message = error.to_string();
                    let _ = shared.dispatch(&HttpEvent {
                        message: message.as_bytes(),
                        ..HttpEvent::bare(HttpEventKind::Error, 0)
                    });
                    let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
                    table.borrow_mut().remove(&shared.resource);
                    return;
                }
            },
            _ = shared.dispatcher.pump(), if !pump_done => { pump_done = true; },
            _ = shared.shutdown.notified() => {
                let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
                table.borrow_mut().remove(&shared.resource);
                return;
            }
        }
    };
    let service = server_service(Rc::clone(&shared));
    // Node's http.Server always allows a half-open socket: a client may shut its write side
    // after the request and still receive the response. HTTP keeps Node's rule.
    let _ = allow_half_close;
    let mut transferred = false;
    let result = if is_http2 {
        let builder = auto::Builder::new(LocalExecutor).http2_only();
        let mut connection = Box::pin(builder.serve_connection(TokioIo::new(io), service));
        let result = tokio::select! {
            biased;
            result = &mut connection => result,
            _ = shared.dispatcher.pump(), if !pump_done => connection.await,
            _ = shared.shutdown.notified() => {
                connection.as_mut().graceful_shutdown();
                connection.await
            }
        };
        result.map_err(|error| error.to_string())
    } else {
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder.auto_date_header(false);
        builder.pipeline_flush(true);
        builder.half_close(true);
        let mut connection = builder.serve_connection(TokioIo::new(io), service);
        enum H1Progress {
            Complete(Result<(), hyper::Error>),
            Pump,
            Shutdown,
        }
        let mut graceful = false;
        let result = loop {
            let progress = tokio::select! {
                biased;
                result = std::future::poll_fn(|cx| connection.poll_without_shutdown(cx)) => H1Progress::Complete(result),
                _ = shared.dispatcher.pump(), if !pump_done => H1Progress::Pump,
                _ = shared.shutdown.notified(), if !graceful => H1Progress::Shutdown,
            };
            match progress {
                H1Progress::Complete(result) => break result,
                H1Progress::Pump => {
                    pump_done = true;
                }
                H1Progress::Shutdown => {
                    Pin::new(&mut connection).graceful_shutdown();
                    graceful = true;
                }
            }
        };
        if result.is_ok() && shared.upgrade_accepted.get() {
            let parts = connection.into_parts();
            let (raw, remaining_prefix) = parts.io.into_inner().into_parts();
            let mut head = parts.read_buf.to_vec();
            head.extend_from_slice(&remaining_prefix);
            let request_id = shared.upgrade_request_id.get();
            let writes = shared.upgrade_writes.borrow_mut().remove(&request_id);
            let pending_write = writes
                .as_ref()
                .map_or_else(Vec::new, |writes| writes.take_trailing());
            let write_completion = writes
                .as_ref()
                .map_or_else(UpgradeWriteCompletion::completed, |writes| {
                    Arc::clone(&writes.completion)
                });
            upgraded.borrow_mut().insert(
                shared.resource,
                UpgradeTransport {
                    transport: HostTransport(raw),
                    head: head.clone(),
                    pending_write,
                    write_completion: Arc::clone(&write_completion),
                },
            );
            let upgrade_dispatched = shared.dispatch(&HttpEvent {
                id: request_id,
                data: &head,
                ..HttpEvent::bare(HttpEventKind::UpgradeReady, request_id)
            });
            if !upgrade_dispatched {
                if let Some(transfer) = upgraded.borrow_mut().remove(&shared.resource) {
                    transfer.write_completion.complete(Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "HTTP upgrade dispatch failed before transport adoption",
                    )));
                } else {
                    transferred = true;
                }
            } else {
                match write_completion.wait().await {
                    Ok(())
                        if writes
                            .as_ref()
                            .is_some_and(|writes| writes.drain_needed.get()) =>
                    {
                        let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::Drain, request_id));
                    }
                    Ok(()) => {}
                    Err(error) => {
                        let message = error.to_string();
                        let _ = shared.dispatch(&HttpEvent {
                            message: message.as_bytes(),
                            ..HttpEvent::bare(HttpEventKind::Error, request_id)
                        });
                    }
                }
                transferred = true;
            }
        }
        result.map_err(|error| error.to_string())
    };
    trace!(
        "{:?} connection future ended: {:?}",
        shared.resource,
        result.as_ref().err()
    );
    if let Err(error) = result {
        let message = error.to_string();
        let event = HttpEvent {
            message: message.as_bytes(),
            ..HttpEvent::bare(HttpEventKind::Error, 0)
        };
        let _ = shared.dispatch(&event);
    }
    let _ = shared.flush_deferred();
    if !transferred {
        let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
    }
    table.borrow_mut().remove(&shared.resource);
}

/// Whether the peer asked to keep the connection: implicit for HTTP/1.1 unless it says
/// `close`, explicit for HTTP/1.0 only when it says `keep-alive`.
fn keep_alive_requested(parts: &hyper::http::request::Parts) -> bool {
    let connection = parts
        .headers
        .get(hyper::header::CONNECTION)
        .and_then(|value| value.to_str().ok());
    let mentions = |token: &str| {
        connection.is_some_and(|value| {
            value
                .split(',')
                .any(|candidate| candidate.trim().eq_ignore_ascii_case(token))
        })
    };
    if parts.version == hyper::Version::HTTP_10 {
        mentions("keep-alive")
    } else {
        !mentions("close")
    }
}

fn request_is_upgrade(request: &Request<Incoming>) -> bool {
    let connection = request
        .headers()
        .get(hyper::header::CONNECTION)
        .and_then(|value| value.to_str().ok());
    let requests_upgrade = connection.is_some_and(|value| {
        value
            .split(',')
            .any(|candidate| candidate.trim().eq_ignore_ascii_case("upgrade"))
    });
    requests_upgrade && request.headers().contains_key(hyper::header::UPGRADE)
}

fn parse_upgrade_response_head(bytes: &[u8]) -> Option<(u16, HeaderMap)> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    let status_line = lines.next()?.strip_suffix(b"\r")?;
    let mut status_parts = status_line.splitn(3, |byte| *byte == b' ');
    let version = status_parts.next()?;
    if version != b"HTTP/1.0" && version != b"HTTP/1.1" {
        return None;
    }
    let status = std::str::from_utf8(status_parts.next()?)
        .ok()?
        .parse::<u16>()
        .ok()?;
    let mut headers = HeaderMap::new();
    for line in lines {
        let line = line.strip_suffix(b"\r")?;
        if line.is_empty() {
            break;
        }
        let colon = line.iter().position(|byte| *byte == b':')?;
        let name = HeaderName::from_bytes(&line[..colon]).ok()?;
        let value = HeaderValue::from_bytes(line[colon + 1..].trim_ascii()).ok()?;
        headers.append(name, value);
    }
    Some((status, headers))
}

async fn handle(
    shared: Rc<ConnectionShared>,
    id: u64,
    request: Request<Incoming>,
) -> Response<EngineBody> {
    let _ = shared.flush_deferred();
    let (parts, mut body) = request.into_parts();
    let http_10 = parts.version == hyper::Version::HTTP_10;
    let head_request = parts.method == hyper::Method::HEAD;
    let is_upgrade = parts.headers.contains_key(hyper::header::UPGRADE)
        && parts
            .headers
            .get(hyper::header::CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|candidate| candidate.trim().eq_ignore_ascii_case("upgrade"))
            });
    // Hyper provides the authoritative post-header bytes when it completes the 101 transfer.
    // Until then the listener receives an empty `head`; those preserved bytes are emitted as
    // the socket's first data event immediately after the listener returns.
    let upgrade_head = [0u8; 0];
    let keep_alive = keep_alive_requested(&parts);
    let slot = Rc::new(ResponseSlot::new(head_request));
    shared.requests.borrow_mut().insert(id, Rc::clone(&slot));
    if is_upgrade {
        shared
            .upgrade_writes
            .borrow_mut()
            .insert(id, Rc::new(UpgradeWrite::new()));
    }

    // An origin-form target is the path and query Hyper already holds; anything else (an
    // absolute-form target from a proxy client) is rendered once.
    let rendered_target;
    let target: &[u8] = match (parts.uri.scheme(), parts.uri.path_and_query()) {
        (None, Some(path)) => path.as_str().as_bytes(),
        _ => {
            rendered_target = parts.uri.to_string();
            rendered_target.as_bytes()
        }
    };
    // A language that takes requests whole gets the body with the head, so the body is read
    // before anything is dispatched. A request that declares no body (every GET) costs nothing.
    let whole = shared.dispatcher.whole_requests() && !is_upgrade;
    let whole_body = if whole && !body.is_end_stream() {
        match (&mut body).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) => {
                trace!("{:?} request {id} body failed: {error}", shared.resource);
                shared.requests.borrow_mut().remove(&id);
                let mut response = Response::new(EngineBody::Empty);
                *response.status_mut() = hyper::StatusCode::BAD_REQUEST;
                response
                    .headers_mut()
                    .insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
                return response;
            }
        }
    } else {
        Bytes::new()
    };
    let host = {
        let mut scratch = shared.header_scratch.borrow_mut();
        if whole {
            flatten_header_wire(&parts.headers, &mut scratch)
        } else {
            flatten_header_map(&parts.headers, &mut scratch);
            None
        }
    };
    let dispatched = {
        let scratch = shared.header_scratch.borrow();
        let mut url_scratch = shared.url_scratch.borrow_mut();
        // A fetch-style handler is given the URL, not the target.
        let target = if whole {
            let host = host.map_or(&b""[..], |range| &scratch[range]);
            request_url(&parts.uri, host, target, &mut url_scratch)
        } else {
            target
        };
        let head = HttpEvent {
            keep_alive,
            version_minor: u8::from(!http_10),
            method: parts.method.as_str().as_bytes(),
            target,
            headers: &scratch,
            data: if is_upgrade {
                &upgrade_head[..]
            } else {
                &whole_body
            },
            ..HttpEvent::bare(
                if is_upgrade {
                    HttpEventKind::UpgradeRequest
                } else {
                    HttpEventKind::RequestHead
                },
                id,
            )
        };
        shared.dispatch(&head)
    };
    let (response, _pump_task) = if dispatched && slot.is_piped_request() {
        slot.ensure_streaming_ready(&shared, id);
        let pump_shared = Rc::clone(&shared);
        let pump_slot = Rc::clone(&slot);
        let pump = tokio::task::spawn_local(async move {
            let mut failed = false;
            while let Some(frame) = body.frame().await {
                match frame {
                    Ok(frame) => {
                        if let Ok(data) = frame.into_data()
                            && !data.is_empty()
                        {
                            let disp = pump_slot.write_bytes(&pump_shared, id, data);
                            if let Some(disp) = disp
                                && !disp.ready
                            {
                                pump_slot.wait_for_drain().await;
                            }
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let event = HttpEvent {
                            message: message.as_bytes(),
                            ..HttpEvent::bare(HttpEventKind::Error, id)
                        };
                        let _ = pump_shared.dispatch(&event);
                        failed = true;
                        break;
                    }
                }
            }
            if !failed {
                pump_slot.end(&pump_shared, id, Bytes::new());
                let _ = pump_shared.dispatch(&HttpEvent::bare(HttpEventKind::RequestEnd, id));
            }
            let _ = pump_shared.flush_deferred();
            pump_shared.requests.borrow_mut().remove(&id);
        });
        let response = if shared.dispatcher.failed() {
            unavailable()
        } else {
            SlotReady {
                slot: Rc::clone(&slot),
            }
            .await
        };
        (response, Some(pump))
    } else {
        if dispatched && !whole && !is_upgrade {
            let mut failed = false;
            while let Some(frame) = body.frame().await {
                match frame {
                    Ok(frame) => {
                        if let Ok(data) = frame.into_data()
                            && !data.is_empty()
                        {
                            let event = HttpEvent {
                                data: &data,
                                ..HttpEvent::bare(HttpEventKind::RequestData, id)
                            };
                            if !shared.dispatch(&event) {
                                failed = true;
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let event = HttpEvent {
                            message: message.as_bytes(),
                            ..HttpEvent::bare(HttpEventKind::Error, id)
                        };
                        let _ = shared.dispatch(&event);
                        failed = true;
                        break;
                    }
                }
            }
            if !failed {
                let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::RequestEnd, id));
            }
        }
        let _ = shared.flush_deferred();

        let response = if shared.dispatcher.failed() {
            unavailable()
        } else {
            SlotReady {
                slot: Rc::clone(&slot),
            }
            .await
        };
        shared.requests.borrow_mut().remove(&id);
        let _ = shared.flush_deferred();
        (response, None)
    };

    let EngineResponse {
        status,
        mut headers,
        body,
    } = response;
    if is_upgrade && status == hyper::StatusCode::SWITCHING_PROTOCOLS.as_u16() {
        shared.upgrade_accepted.set(true);
        shared.upgrade_request_id.set(id);
    } else if is_upgrade {
        shared.upgrade_writes.borrow_mut().remove(&id);
    }
    let is_http2 = parts.version == hyper::Version::HTTP_2;
    let close_delimited = http_10
        && !head_request
        && status_allows_body(status)
        && !headers.contains_key(hyper::header::CONTENT_LENGTH);
    if is_http2 {
        // RFC 7540 §8.1.2.2: Connection-specific headers MUST NOT appear in HTTP/2 messages.
        headers.remove(hyper::header::CONNECTION);
        headers.remove(hyper::header::TRANSFER_ENCODING);
        headers.remove(hyper::header::UPGRADE);
        headers.remove("keep-alive");
        headers.remove("proxy-connection");
    } else if !headers.contains_key(hyper::header::CONNECTION)
        && (!keep_alive || close_delimited || shared.closing.get())
    {
        headers.insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
    }
    if shared.http3
        && !headers.contains_key("alt-svc")
        && let Ok(val) = HeaderValue::from_str("h3=\":443\"; ma=86400")
    {
        headers.insert(hyper::header::HeaderName::from_static("alt-svc"), val);
    }
    let mut hyper_response = Response::new(body);
    *hyper_response.status_mut() =
        hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    if close_delimited {
        // Hyper decides keep-alive against the response's version, so a close-delimited body
        // has to say HTTP/1.0 for `connection: close` to survive.
        *hyper_response.version_mut() = hyper::Version::HTTP_10;
    }
    *hyper_response.headers_mut() = headers;
    hyper_response
}

fn unavailable() -> EngineResponse {
    let mut headers = HeaderMap::new();
    headers.insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
    EngineResponse {
        status: 503,
        headers,
        body: EngineBody::Empty,
    }
}

/// A whole request's URL, for a fetch-style handler: an origin-form target (a path, or `*`)
/// behind `http://` and the Host field, so the language concatenates nothing. Any other target
/// (absolute-form from a proxy client, or an HTTP/2 request, whose URI carries its scheme and
/// authority; authority-form) is handed over as rendered, and so is an origin-form target with no
/// Host field (HTTP/1.0), for the language to complete with the listening authority -- it is the
/// one case that starts with `/` or `*`.
///
/// `host` is the first Host field's value, empty when there is none.
fn request_url<'a>(
    uri: &hyper::Uri,
    host: &[u8],
    target: &'a [u8],
    into: &'a mut Vec<u8>,
) -> &'a [u8] {
    if host.is_empty() || uri.scheme().is_some() || uri.path_and_query().is_none() {
        return target;
    }
    into.clear();
    into.reserve(7 + host.len() + target.len());
    into.extend_from_slice(b"http://");
    into.extend_from_slice(host);
    into.extend_from_slice(target);
    into
}

/// The header-list wire form both directions of a fetch-style connection use: `\nname:value`
/// per field, in map order, with nothing before the first field or after the last. Hyper's names
/// are lowercase tokens and its values hold no CR, LF or NUL, so a field starts exactly where
/// `\n` + name + `:` occurs, and a value runs to the next `\n`.
///
/// Answers where the first Host field's value lies in `into`, which the request's URL is built
/// from, so the map is not searched for it again.
pub(crate) fn flatten_header_wire(
    headers: &HeaderMap,
    into: &mut Vec<u8>,
) -> Option<std::ops::Range<usize>> {
    into.clear();
    let needed: usize = headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 2)
        .sum();
    into.reserve(needed);
    let mut host = None;
    for (name, value) in headers {
        into.push(b'\n');
        into.extend_from_slice(name.as_str().as_bytes());
        into.push(b':');
        let start = into.len();
        into.extend_from_slice(value.as_bytes());
        if host.is_none() && name == hyper::header::HOST {
            host = Some(start..into.len());
        }
    }
    host
}

/// [`flatten_header_wire`]'s form back into a header map, refusing anything Hyper would refuse
/// (a name that is not a token, a value with CR, LF or NUL) and a block that does not start
/// with a field separator. The value is everything after the first `:`, taken as is: the
/// language normalized it already.
pub(crate) fn parse_header_wire(bytes: &[u8]) -> Option<HeaderMap> {
    let Some(rest) = bytes.strip_prefix(b"\n") else {
        return bytes.is_empty().then(HeaderMap::new);
    };
    let mut headers = HeaderMap::new();
    for field in rest.split(|byte| *byte == b'\n') {
        let colon = field.iter().position(|byte| *byte == b':')?;
        let name = HeaderName::from_bytes(&field[..colon]).ok()?;
        let value = HeaderValue::from_bytes(&field[colon + 1..]).ok()?;
        headers.append(name, value);
    }
    Some(headers)
}

pub(crate) fn flatten_header_map(headers: &HeaderMap, into: &mut Vec<u8>) {
    into.clear();
    let needed: usize = headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 2)
        .sum();
    into.reserve(needed);
    for (name, value) in headers {
        into.extend_from_slice(name.as_str().as_bytes());
        into.push(0);
        into.extend_from_slice(value.as_bytes());
        into.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Recording {
        events: RefCell<Vec<(HttpEventKind, u64, Vec<u8>)>>,
        checkpoints: Cell<usize>,
        answer: RefCell<Option<Box<dyn Fn(u64)>>>,
        answer_head: RefCell<Option<Box<dyn Fn(u64)>>>,
    }

    impl HttpDispatch for Recording {
        fn event(&self, event: &HttpEvent<'_>) -> i32 {
            let payload = match event.kind {
                HttpEventKind::RequestHead => {
                    let mut bytes = event.method.to_vec();
                    bytes.push(b' ');
                    bytes.extend_from_slice(event.target);
                    bytes.push(b' ');
                    bytes.extend_from_slice(event.headers);
                    bytes
                }
                HttpEventKind::RequestData => event.data.to_vec(),
                HttpEventKind::Error => event.message.to_vec(),
                _ => Vec::new(),
            };
            self.events
                .borrow_mut()
                .push((event.kind, event.id, payload));
            if event.kind == HttpEventKind::RequestHead
                && let Some(answer) = self.answer_head.borrow().as_ref()
            {
                answer(event.id);
            }
            if event.kind == HttpEventKind::RequestEnd
                && let Some(answer) = self.answer.borrow().as_ref()
            {
                answer(event.id);
            }
            0
        }

        fn checkpoint(&self) -> i32 {
            self.checkpoints.set(self.checkpoints.get() + 1);
            0
        }
    }

    fn local() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime builds");
        (runtime, tokio::task::LocalSet::new())
    }

    async fn read_until_complete(client: &mut tokio::io::DuplexStream, ends_with: &[u8]) -> String {
        let mut answer = Vec::new();
        let mut buffer = [0u8; 1024];
        for _ in 0..64 {
            let read = client.read(&mut buffer).await.expect("response reads");
            answer.extend_from_slice(&buffer[..read]);
            if answer.ends_with(ends_with) || read == 0 {
                break;
            }
        }
        String::from_utf8(answer).expect("utf-8 response")
    }

    /// A dispatch that takes requests whole and answers each from inside its head event.
    struct Whole {
        engine: Rc<RefCell<HttpServerEngine>>,
        resource: ResourceId,
        events: RefCell<Vec<(HttpEventKind, Vec<u8>)>>,
        /// Each head's target (the URL, on a whole-request connection) and header block.
        heads: RefCell<Vec<(Vec<u8>, Vec<u8>)>>,
    }

    impl HttpDispatch for Whole {
        fn event(&self, event: &HttpEvent<'_>) -> i32 {
            self.events
                .borrow_mut()
                .push((event.kind, event.data.to_vec()));
            if event.kind == HttpEventKind::RequestHead {
                self.heads
                    .borrow_mut()
                    .push((event.target.to_vec(), event.headers.to_vec()));
                let mut engine = self.engine.borrow_mut();
                // The NUL form is not this connection's: refused, and the slot is still open.
                assert!(!engine.start_response(self.resource, event.id, 201, b"x-n\x001\0", 1));
                assert!(engine.start_response(self.resource, event.id, 201, b"\nx-n:1\nx-n:2", 1));
                let mut body = b"got ".to_vec();
                body.extend_from_slice(event.data);
                engine
                    .end_response(self.resource, event.id, &body)
                    .expect("response ends");
            }
            0
        }

        fn checkpoint(&self) -> i32 {
            0
        }

        fn whole_requests(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_whole_request_arrives_as_one_event_carrying_its_body() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (mut client, server) = tokio::io::duplex(1 << 16);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let resource = ResourceId::new(9);
            let whole = Rc::new(Whole {
                engine: Rc::clone(&engine),
                resource,
                events: RefCell::new(Vec::new()),
                heads: RefCell::new(Vec::new()),
            });
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                whole.clone(),
            ));
            // A body in two chunks (chunked encoding) is collected before the head is dispatched.
            client
                .write_all(
                    b"POST /a HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\n\r\n\
                      3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
                )
                .await
                .expect("request writes");
            let text = read_until_complete(&mut client, b"got abcde").await;
            assert!(text.starts_with("HTTP/1.1 201 Created\r\n"), "{text}");
            assert!(text.contains("x-n: 1\r\nx-n: 2\r\n"), "{text}");
            // A request without a body costs no wait at all.
            client
                .write_all(b"GET /b HTTP/1.1\r\nhost: x\r\n\r\n")
                .await
                .expect("second request writes");
            let text = read_until_complete(&mut client, b"got ").await;
            assert!(text.contains("content-length: 4\r\n"), "{text}");
            drop(client);
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let events = whole.events.borrow();
            let heads: Vec<_> = events
                .iter()
                .filter(|(kind, _)| *kind == HttpEventKind::RequestHead)
                .map(|(_, data)| data.clone())
                .collect();
            assert_eq!(heads, vec![b"abcde".to_vec(), Vec::new()]);
            // The handler is given the URL and the header list in wire form.
            assert_eq!(
                *whole.heads.borrow(),
                vec![
                    (
                        b"http://x/a".to_vec(),
                        b"\nhost:x\ntransfer-encoding:chunked".to_vec()
                    ),
                    (b"http://x/b".to_vec(), b"\nhost:x".to_vec()),
                ]
            );
            assert!(
                events.iter().all(|(kind, _)| !matches!(
                    kind,
                    HttpEventKind::RequestData | HttpEventKind::RequestEnd
                )),
                "{:?}",
                events.iter().map(|(kind, _)| *kind).collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn a_request_is_dispatched_directly_and_answered_through_the_slot() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (mut client, server) = tokio::io::duplex(4096);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
                answer_head: RefCell::new(None),
            });
            let resource = ResourceId::new(7);
            let answer_engine = Rc::clone(&engine);
            *recording.answer.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(engine.start_response(
                    resource,
                    id,
                    200,
                    b"content-type\0text/plain\0",
                    1024,
                ));
                let disposition = engine
                    .end_response(resource, id, b"kiln")
                    .expect("response ends");
                assert!(disposition.ready);
            }));
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));

            client
                .write_all(b"GET /hello?x=1 HTTP/1.1\r\nhost: x\r\nx-a: 1\r\n\r\n")
                .await
                .expect("request writes");
            let text = read_until_complete(&mut client, b"kiln").await;
            assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
            assert!(text.contains("content-length: 4\r\n"), "{text}");
            assert!(!text.contains("transfer-encoding"), "{text}");

            // A second request on the same connection reuses the scratch buffer and the slot
            // table; a body-less answer declares length zero.
            let answer_engine = Rc::clone(&engine);
            *recording.answer.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(engine.start_response(resource, id, 200, b"", 1024));
                engine
                    .end_response(resource, id, b"")
                    .expect("empty response ends");
            }));
            client
                .write_all(b"GET /second HTTP/1.1\r\nhost: y\r\n\r\n")
                .await
                .expect("second request writes");
            let text = read_until_complete(&mut client, b"\r\n\r\n").await;
            assert!(text.contains("content-length: 0\r\n"), "{text}");

            drop(client);
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let events = recording.events.borrow();
            let kinds: Vec<_> = events.iter().map(|(kind, _, _)| *kind).collect();
            assert_eq!(
                kinds,
                vec![
                    HttpEventKind::RequestHead,
                    HttpEventKind::RequestEnd,
                    HttpEventKind::Finish,
                    HttpEventKind::RequestHead,
                    HttpEventKind::RequestEnd,
                    HttpEventKind::Finish,
                    HttpEventKind::Closed,
                ],
                "{events:?}"
            );
            assert_eq!(
                events[0].2,
                b"GET /hello?x=1 host\0x\0x-a\x001\0".to_vec(),
                "head carries method, target and NUL-flattened headers"
            );
            assert_eq!(events[3].2, b"GET /second host\0y\0".to_vec());
            assert_eq!(recording.checkpoints.get(), events.len());
            assert_eq!(engine.borrow().open_connections(), 0);
        });
    }

    #[test]
    fn an_invalid_header_refuses_the_head_and_a_streamed_body_is_chunked() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (mut client, server) = tokio::io::duplex(4096);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
                answer_head: RefCell::new(None),
            });
            let resource = ResourceId::new(9);
            let answer_engine = Rc::clone(&engine);
            *recording.answer.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(
                    !engine.start_response(resource, id, 200, b"bad name\0v\0", 1024),
                    "a header name with a space is refused"
                );
                assert!(engine.start_response(resource, id, 200, b"x-k\0v\0", 1024));
                engine
                    .write_response(resource, id, b"ab")
                    .expect("first chunk");
                engine
                    .end_response(resource, id, b"cd")
                    .expect("last chunk");
            }));
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));
            client
                .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
                .await
                .expect("request writes");
            let text = read_until_complete(&mut client, b"0\r\n\r\n").await;
            assert!(text.contains("transfer-encoding: chunked\r\n"), "{text}");
            assert!(text.contains("x-k: v\r\n"), "{text}");
            assert!(text.ends_with("2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n"), "{text}");
        });
    }

    #[test]
    fn static_response_serves_direct_bytes_with_content_length() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (mut client, server) = tokio::io::duplex(4096);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
                answer_head: RefCell::new(None),
            });
            let resource = ResourceId::new(10);
            let answer_engine = Rc::clone(&engine);
            *recording.answer.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                let body = Bytes::from_static(b"static payload direct bytes");
                let headers = b"content-type\0text/plain\0";
                engine
                    .send_static_response(resource, id, 200, headers, body)
                    .expect("static response sends");
            }));
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));
            client
                .write_all(b"GET /static HTTP/1.1\r\nhost: test\r\n\r\n")
                .await
                .expect("request writes");
            let text = read_until_complete(&mut client, b"static payload direct bytes").await;
            assert!(text.contains("content-length: 27\r\n"), "{text}");
            assert!(text.contains("content-type: text/plain\r\n"), "{text}");
            assert!(
                text.ends_with("\r\n\r\nstatic payload direct bytes"),
                "{text}"
            );
        });
    }

    #[test]
    fn piped_request_runs_directly_in_rust_without_chunk_events_to_js() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (mut client, server) = tokio::io::duplex(4096);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
                answer_head: RefCell::new(None),
            });
            let resource = ResourceId::new(11);
            let answer_engine = Rc::clone(&engine);
            *recording.answer_head.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(engine.pipe_request(resource, id));
            }));
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));

            client
                .write_all(b"POST /pipe HTTP/1.1\r\nhost: test\r\ncontent-length: 15\r\n\r\n")
                .await
                .expect("request head writes");
            client
                .write_all(b"hello from pipe")
                .await
                .expect("request body writes");

            let text = read_until_complete(&mut client, b"0\r\n\r\n").await;
            assert!(text.contains("HTTP/1.1 200 OK\r\n"), "{text}");
            assert!(text.contains("transfer-encoding: chunked\r\n"), "{text}");
            assert!(text.contains("hello from pipe"), "{text}");

            let events = recording.events.borrow();
            let kinds: Vec<_> = events.iter().map(|(kind, _, _)| *kind).collect();
            // Crucial assertion: RequestData must NEVER be dispatched to JS when piped!
            assert!(
                !kinds.contains(&HttpEventKind::RequestData),
                "leaked chunk events: {kinds:?}"
            );
            assert!(kinds.contains(&HttpEventKind::RequestHead));
            assert!(kinds.contains(&HttpEventKind::RequestEnd));
            assert!(kinds.contains(&HttpEventKind::Finish));
        });
    }

    #[test]
    fn http2_cleartext_request_is_served_and_multiplexed() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (client, server) = tokio::io::duplex(8192);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
                answer_head: RefCell::new(None),
            });
            let resource = ResourceId::new(42);
            let answer_engine = Rc::clone(&engine);
            *recording.answer.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(engine.start_response(
                    resource,
                    id,
                    200,
                    b"content-type\0text/plain\0",
                    1024,
                ));
                let body = format!("h2-response-{id}");
                let disposition = engine
                    .end_response(resource, id, body.as_bytes())
                    .expect("response ends");
                assert!(disposition.ready);
            }));

            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));

            let (mut send_request, client_conn) = h2::client::handshake(client)
                .await
                .expect("h2 client handshake");
            tokio::task::spawn_local(async move {
                let _ = client_conn.await;
            });

            // Send first HTTP/2 request
            let request1 = hyper::http::Request::builder()
                .method("GET")
                .uri("http://localhost/h2-first")
                .body(())
                .unwrap();
            let (response1, _send_stream1) = send_request
                .send_request(request1, true)
                .expect("send request 1");

            // Send second HTTP/2 request multiplexed on the same connection
            let request2 = hyper::http::Request::builder()
                .method("POST")
                .uri("http://localhost/h2-second")
                .body(())
                .unwrap();
            let (response2, mut send_stream2) = send_request
                .send_request(request2, false)
                .expect("send request 2");
            send_stream2
                .send_data(Bytes::from_static(b"req2-data"), true)
                .expect("send data 2");

            let (parts1, mut body1) = response1.await.expect("response 1 arrives").into_parts();
            assert_eq!(parts1.status, 200);
            assert_eq!(parts1.headers.get("content-type").unwrap(), "text/plain");
            // RFC 7540: connection-specific headers must NOT be present in HTTP/2 response
            assert!(!parts1.headers.contains_key("connection"));
            let mut body_bytes1 = Vec::new();
            while let Some(chunk) = body1.data().await {
                body_bytes1.extend_from_slice(&chunk.expect("chunk 1"));
            }
            assert_eq!(&body_bytes1[..], b"h2-response-1");

            let (parts2, mut body2) = response2.await.expect("response 2 arrives").into_parts();
            assert_eq!(parts2.status, 200);
            assert!(!parts2.headers.contains_key("connection"));
            let mut body_bytes2 = Vec::new();
            while let Some(chunk) = body2.data().await {
                body_bytes2.extend_from_slice(&chunk.expect("chunk 2"));
            }
            assert_eq!(&body_bytes2[..], b"h2-response-2");

            let events = recording.events.borrow();
            let request_heads: Vec<_> = events
                .iter()
                .filter(|(kind, _, _)| *kind == HttpEventKind::RequestHead)
                .collect();
            assert_eq!(request_heads.len(), 2);
        });
    }

    #[test]
    fn http3_flag_and_alt_svc_advertising() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (mut client, server) = tokio::io::duplex(4096);
            let engine = Rc::new(RefCell::new(HttpServerEngine::with_http3(true)));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
                answer_head: RefCell::new(None),
            });
            let resource = ResourceId::new(99);
            let answer_engine = Rc::clone(&engine);
            *recording.answer.borrow_mut() = Some(Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(engine.start_response(
                    resource,
                    id,
                    200,
                    b"content-type\0text/plain\0",
                    1024,
                ));
                let disposition = engine
                    .end_response(resource, id, b"http3-test")
                    .expect("response ends");
                assert!(disposition.ready);
            }));
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));

            client
                .write_all(b"GET /h3-probe HTTP/1.1\r\nhost: localhost\r\n\r\n")
                .await
                .expect("request writes");
            let text = read_until_complete(&mut client, b"http3-test").await;
            assert!(
                text.contains("alt-svc: h3=\":443\"; ma=86400"),
                "alt-svc header missing: {text}"
            );
        });
    }

    #[test]
    fn http3_configuration_is_owned_by_each_engine() {
        let http3 = HttpServerEngine::with_http3(true);
        let http1 = HttpServerEngine::with_http3(false);
        assert!(http3.http3);
        assert!(!http1.http3);
    }

    #[test]
    fn the_header_wire_form_round_trips_and_refuses_what_hyper_refuses() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com:80"));
        headers.append("set-cookie", HeaderValue::from_static("a=1"));
        headers.append("set-cookie", HeaderValue::from_static("b=2; Path=/"));
        headers.insert("x-empty", HeaderValue::from_static(""));
        let mut wire = Vec::new();
        let host = flatten_header_wire(&headers, &mut wire).expect("a Host field");
        assert_eq!(&wire[host], b"example.com:80");
        assert_eq!(
            wire,
            b"\nhost:example.com:80\nset-cookie:a=1\nset-cookie:b=2; Path=/\nx-empty:".to_vec()
        );
        assert_eq!(parse_header_wire(&wire), Some(headers));
        assert_eq!(parse_header_wire(b""), Some(HeaderMap::new()));
        // Names are taken as tokens (and lowercased by Hyper); a value keeps every byte.
        let parsed = parse_header_wire(b"\nContent-Type:text/plain; a=b:c").expect("parses");
        assert_eq!(parsed["content-type"], "text/plain; a=b:c");
        for refused in [
            &b"host:x"[..],
            b"\nno-colon",
            b"\nbad name:v",
            b"\n:v",
            b"\nx:a\rb",
            b"\nx:a\0b",
            b"\nx:1\n",
        ] {
            assert_eq!(parse_header_wire(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn a_whole_requests_url_is_built_from_the_host_field() {
        let parts = |head: &str| {
            let mut request = Request::builder();
            let mut lines = head.split('\n');
            let (method, uri) = lines.next().expect("line").split_once(' ').expect("target");
            request = request.method(method).uri(uri);
            for line in lines {
                let (name, value) = line.split_once(':').expect("field");
                request = request.header(name, value);
            }
            request.body(()).expect("request builds").into_parts().0
        };
        let url = |head: &str| {
            let parts = parts(head);
            let target = match (parts.uri.scheme(), parts.uri.path_and_query()) {
                (None, Some(path)) => path.as_str().to_owned(),
                _ => parts.uri.to_string(),
            };
            let mut wire = Vec::new();
            let host = flatten_header_wire(&parts.headers, &mut wire);
            let host = host.map_or(&b""[..], |range| &wire[range]);
            let mut scratch = Vec::new();
            String::from_utf8(
                request_url(&parts.uri, host, target.as_bytes(), &mut scratch).to_vec(),
            )
            .expect("utf-8")
        };
        assert_eq!(url("GET /a?b=1\nhost:h:81"), "http://h:81/a?b=1");
        assert_eq!(url("OPTIONS *\nhost:h"), "http://h*");
        // No Host field (HTTP/1.0), or an empty one: the target, for the language to complete.
        assert_eq!(url("GET /a"), "/a");
        assert_eq!(url("GET /a\nhost:"), "/a");
        // An absolute-form target is the URL already, whatever Host says.
        assert_eq!(url("GET http://p.test/x\nhost:h"), "http://p.test/x");
    }

    #[test]
    fn flatten_header_map_reserves_capacity_and_encodes_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(hyper::header::HOST, HeaderValue::from_static("example.com"));
        headers.insert(
            hyper::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let mut buffer = Vec::with_capacity(16);
        flatten_header_map(&headers, &mut buffer);
        let expected_needed = "host".len()
            + "example.com".len()
            + 2
            + "content-type".len()
            + "application/json".len()
            + 2;
        assert!(buffer.capacity() >= expected_needed);
        let text = std::str::from_utf8(&buffer).expect("valid utf-8");
        assert!(text.contains("host\0example.com\0"));
        assert!(text.contains("content-type\0application/json\0"));
    }
}

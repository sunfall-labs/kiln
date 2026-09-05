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
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Frame, Incoming};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use kiln_host_abi::ResourceId;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Notify;

use crate::HttpCloseMode;
use crate::http_body::{BodyEvent, BodySink, ResponseBody, ResponseBodyDriver};
use crate::node_stream::WriteDisposition;

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
    /// `name NUL value NUL` per header, the shape the private C ABI already consumes.
    pub headers: &'a [u8],
    pub data: &'a [u8],
    pub message: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpEventKind {
    RequestHead,
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
    fn event(&self, event: &HttpEvent<'_>) -> i32;
    fn checkpoint(&self) -> i32;
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
}

impl Dispatcher {
    pub(crate) fn new(resource: ResourceId, dispatch: Rc<dyn HttpDispatch>) -> Self {
        Self {
            resource,
            dispatch,
            deferred: RefCell::new(Vec::new()),
            deferred_wake: Notify::new(),
            failed: Cell::new(false),
        }
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.get()
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
#[derive(Default)]
pub struct HttpServerEngine {
    connections: Rc<RefCell<HashMap<ResourceId, ServerConnection>>>,
}

struct ServerConnection {
    shared: Rc<ConnectionShared>,
    task: tokio::task::JoinHandle<()>,
}

struct ConnectionShared {
    resource: ResourceId,
    dispatcher: Rc<Dispatcher>,
    requests: RefCell<HashMap<u64, Rc<ResponseSlot>>>,
    /// The flattened request headers of the request being dispatched, reused per request.
    header_scratch: RefCell<Vec<u8>>,
    shutdown: Notify,
    closing: Cell<bool>,
    /// An abort has been requested and its `Closed` has not been published yet. Counted as
    /// work in flight so a drive does not return before the language hears it.
    abort_pending: Cell<bool>,
}

impl ConnectionShared {
    fn dispatch(&self, event: &HttpEvent<'_>) -> bool {
        self.dispatcher.dispatch(event)
    }

    fn defer(&self, id: u64, event: BodyEvent) {
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

    fn write(
        &self,
        shared: &Rc<ConnectionShared>,
        id: u64,
        data: &[u8],
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
                let disposition = driver.write(data.to_vec());
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
                Some(driver.write(data.to_vec()))
            }
            SlotState::Streaming { driver, .. } => Some(driver.write(data.to_vec())),
            SlotState::Empty
            | SlotState::Ready {
                body: ResponseBodyState::Complete(_),
                ..
            }
            | SlotState::Finished => None,
        }
    }

    fn end(&self, shared: &Rc<ConnectionShared>, id: u64, data: &[u8]) -> Option<WriteDisposition> {
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
                    Bytes::copy_from_slice(data)
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
                let disposition = driver.end(data.to_vec());
                if *bodyless {
                    driver.finish_without_body();
                }
                *ended = true;
                Some(disposition)
            }
            SlotState::Streaming { driver, bodyless } => {
                let disposition = driver.end(data.to_vec());
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
            header_scratch: RefCell::new(Vec::with_capacity(512)),
            shutdown: Notify::new(),
            closing: Cell::new(false),
            abort_pending: Cell::new(false),
        });
        trace!("{resource:?} attach half_close={allow_half_close}");
        let table = Rc::clone(&self.connections);
        let task = tokio::task::spawn_local(serve(
            transport,
            allow_half_close,
            Rc::clone(&shared),
            table,
        ));
        self.connections
            .borrow_mut()
            .insert(resource, ServerConnection { shared, task });
        true
    }

    pub fn is_attached(&self, resource: ResourceId) -> bool {
        self.connections.borrow().contains_key(&resource)
    }

    /// Record the head of a response. `headers` is the private ABI's `name NUL value NUL`
    /// flattening; an invalid name or value refuses the head.
    pub fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: &[u8],
        high_water_mark: usize,
    ) -> bool {
        let Some(slot) = self.slot(resource, id) else {
            return false;
        };
        let Some(headers) = parse_flattened_headers(headers) else {
            return false;
        };
        slot.start(status, headers, high_water_mark)
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

    pub fn end_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.end(&shared, id, data)
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

    fn slot(&self, resource: ResourceId, id: u64) -> Option<Rc<ResponseSlot>> {
        let table = self.connections.borrow();
        let connection = table.get(&resource)?;
        let requests = connection.shared.requests.borrow();
        requests.get(&id).cloned()
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

async fn serve(
    transport: HostTransport,
    allow_half_close: bool,
    shared: Rc<ConnectionShared>,
    table: Rc<RefCell<HashMap<ResourceId, ServerConnection>>>,
) {
    let next_id = Cell::new(1_u64);
    let service_shared = Rc::clone(&shared);
    let service = service_fn(move |request: Request<Incoming>| {
        let id = next_id.get();
        next_id.set(id.wrapping_add(1).max(1));
        let shared = Rc::clone(&service_shared);
        async move { Ok::<_, std::convert::Infallible>(handle(shared, id, request).await) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.auto_date_header(false);
    // Node's http.Server always allows a half-open socket: a client may shut its write side
    // after the request and still receive the response. With the socket in Hyper's hands the
    // EOF is seen at once, so refusing half-close would drop every such response before it is
    // written (the bridged engine only surfaced the EOF a round trip later). The option a
    // program passes still selects raw-socket behaviour elsewhere; HTTP keeps Node's rule.
    let _ = allow_half_close;
    builder.half_close(true);
    let mut connection = Box::pin(builder.serve_connection(TokioIo::new(transport.0), service));
    let result = tokio::select! {
        biased;
        result = &mut connection => result,
        _ = shared.dispatcher.pump() => connection.await,
        _ = shared.shutdown.notified() => {
            connection.as_mut().graceful_shutdown();
            connection.await
        }
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
    let _ = shared.dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
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

async fn handle(
    shared: Rc<ConnectionShared>,
    id: u64,
    request: Request<Incoming>,
) -> Response<EngineBody> {
    let _ = shared.flush_deferred();
    let (parts, mut body) = request.into_parts();
    let http_10 = parts.version == hyper::Version::HTTP_10;
    let head_request = parts.method == hyper::Method::HEAD;
    let keep_alive = keep_alive_requested(&parts);
    let slot = Rc::new(ResponseSlot::new(head_request));
    shared.requests.borrow_mut().insert(id, Rc::clone(&slot));

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
    {
        let mut scratch = shared.header_scratch.borrow_mut();
        flatten_header_map(&parts.headers, &mut scratch);
    }
    let dispatched = {
        let scratch = shared.header_scratch.borrow();
        let head = HttpEvent {
            keep_alive,
            version_minor: u8::from(!http_10),
            method: parts.method.as_str().as_bytes(),
            target,
            headers: &scratch,
            ..HttpEvent::bare(HttpEventKind::RequestHead, id)
        };
        shared.dispatch(&head)
    };
    if dispatched {
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

    let EngineResponse {
        status,
        mut headers,
        body,
    } = response;
    let close_delimited = http_10
        && !head_request
        && status_allows_body(status)
        && !headers.contains_key(hyper::header::CONTENT_LENGTH);
    if !headers.contains_key(hyper::header::CONNECTION)
        && (!keep_alive || close_delimited || shared.closing.get())
    {
        headers.insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
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

pub(crate) fn flatten_header_map(headers: &HeaderMap, into: &mut Vec<u8>) {
    into.clear();
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
}

//! HTTP server connections as Hyper tasks over host transports (ADR 0042).
//!
//! A connection is one Hyper future over a transport the host handed over. Its service calls the
//! realm's adapter directly, on the realm thread, inside the task's poll; the adapter answers
//! through a [`ResponseSlot`] the service future awaits. Nothing is spawned per read, queued per
//! event or re-sequenced per completion. Body events that would otherwise arrive inside the
//! language's own call (`finish` from within `res.end()`) are deferred to the next safe point so
//! the language never observes a callback from inside another.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use kiln_host_abi::ResourceId;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Notify;

use crate::http_body::{BodyEvent, BodySink, ResponseBody, ResponseBodyDriver};
use crate::hyper_http::{
    ResponseCommand, declares_content_length, hyper_response, kiln_request_parts,
    response_status_allows_body,
};
use crate::node_http::HttpVersion;
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
        if trace_enabled() {
            eprintln!("[kiln-http] {}", format_args!($($arg)*));
        }
    };
}

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

/// One protocol event delivered to the language adapter. Every span is borrowed for the call.
#[derive(Debug, Clone, Copy)]
pub struct ServerEvent<'a> {
    pub kind: ServerEventKind,
    pub id: u64,
    pub keep_alive: bool,
    pub version_minor: u8,
    pub method: &'a [u8],
    pub target: &'a [u8],
    /// `name NUL value NUL` per header, the shape the private C ABI already consumes.
    pub headers: &'a [u8],
    pub data: &'a [u8],
    pub message: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerEventKind {
    RequestHead,
    RequestData,
    RequestEnd,
    Drain,
    Finish,
    Error,
    Closed,
}

impl ServerEvent<'static> {
    fn bare(kind: ServerEventKind, id: u64) -> Self {
        Self {
            kind,
            id,
            keep_alive: false,
            version_minor: 0,
            method: &[],
            target: &[],
            headers: &[],
            data: &[],
            message: &[],
        }
    }
}

/// The realm's side of a connection. `event` runs the language; `checkpoint` drains what the
/// language queued (next-ticks, microtasks, the rejection check) before the engine continues.
/// A non-zero return is a realm failure and ends the connection.
pub trait ServerDispatch {
    fn event(&self, event: &ServerEvent<'_>) -> i32;
    fn checkpoint(&self) -> i32;
}

/// How a connection should stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerCloseMode {
    /// Finish the response in flight, then close.
    Graceful,
    /// Drop the transport now.
    Abort,
}

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
    dispatch: Rc<dyn ServerDispatch>,
    requests: RefCell<HashMap<u64, Rc<ResponseSlot>>>,
    /// Body events (`drain`, `finish`) produced inside a language call, delivered after it.
    deferred: RefCell<Vec<(u64, BodyEvent)>>,
    deferred_wake: Notify,
    shutdown: Notify,
    closing: Cell<bool>,
    failed: Cell<bool>,
}

impl ConnectionShared {
    fn dispatch(&self, event: &ServerEvent<'_>) -> bool {
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

    fn defer(&self, id: u64, event: BodyEvent) {
        self.deferred.borrow_mut().push((id, event));
        self.deferred_wake.notify_one();
    }

    fn flush_deferred(&self) -> bool {
        loop {
            let pending = std::mem::take(&mut *self.deferred.borrow_mut());
            if pending.is_empty() {
                return true;
            }
            for (id, event) in pending {
                let kind = match event {
                    BodyEvent::Drain => ServerEventKind::Drain,
                    BodyEvent::Finish => ServerEventKind::Finish,
                };
                if !self.dispatch(&ServerEvent::bare(kind, id)) {
                    return false;
                }
            }
        }
    }
}

/// Where a response accumulates until the service future can hand it to Hyper.
pub struct ResponseSlot {
    inner: RefCell<SlotInner>,
}

struct SlotInner {
    head_request: bool,
    /// The head as the language gave it, parked until framing is knowable.
    head: Option<(u16, Vec<(String, String)>)>,
    body: Option<ResponseBodyDriver>,
    high_water_mark: usize,
    /// The head is committed: a write forced chunking, or `end` declared the length.
    flushed: Option<(u16, Vec<(String, String)>)>,
    bodyless: bool,
    waker: Option<Waker>,
}

impl ResponseSlot {
    fn new(head_request: bool) -> Self {
        Self {
            inner: RefCell::new(SlotInner {
                head_request,
                head: None,
                body: None,
                high_water_mark: crate::node_stream::DEFAULT_HIGH_WATER_MARK,
                flushed: None,
                bodyless: false,
                waker: None,
            }),
        }
    }

    fn start(&self, status: u16, headers: Vec<(String, String)>, high_water_mark: usize) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner.head.is_some() || inner.flushed.is_some() {
            return false;
        }
        inner.head = Some((status, headers));
        inner.high_water_mark = high_water_mark.max(1);
        true
    }

    /// Commit the parked head. A complete body declares its length unless the language framed
    /// the response itself; a HEAD answer or a bodiless status never carries a body.
    fn flush(inner: &mut SlotInner, complete_body: Option<usize>) -> bool {
        if inner.flushed.is_some() {
            return true;
        }
        let Some((status, mut headers)) = inner.head.take() else {
            return false;
        };
        let carries_body = response_status_allows_body(status) && !inner.head_request;
        inner.bodyless = !carries_body;
        if let Some(length) = complete_body
            && carries_body
            && !declares_framing(&headers)
        {
            headers.push(("content-length".into(), length.to_string()));
        }
        inner.flushed = Some((status, headers));
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
        true
    }

    fn write(
        &self,
        shared: &Rc<ConnectionShared>,
        id: u64,
        data: Vec<u8>,
    ) -> Option<WriteDisposition> {
        let mut inner = self.inner.borrow_mut();
        if !Self::flush(&mut inner, None) {
            return None;
        }
        let high_water_mark = inner.high_water_mark;
        let body = inner
            .body
            .get_or_insert_with(|| body_driver(shared, id, high_water_mark));
        Some(body.write(data))
    }

    fn end(
        &self,
        shared: &Rc<ConnectionShared>,
        id: u64,
        data: Vec<u8>,
    ) -> Option<WriteDisposition> {
        let mut inner = self.inner.borrow_mut();
        if !Self::flush(&mut inner, Some(data.len())) {
            return None;
        }
        let bodyless = inner.bodyless;
        let high_water_mark = inner.high_water_mark;
        let body = inner
            .body
            .get_or_insert_with(|| body_driver(shared, id, high_water_mark));
        let disposition = body.end(data);
        if bodyless {
            body.finish_without_body();
        }
        Some(disposition)
    }
}

fn body_driver(
    shared: &Rc<ConnectionShared>,
    id: u64,
    high_water_mark: usize,
) -> ResponseBodyDriver {
    let driver = ResponseBodyDriver::new(high_water_mark);
    let sink = Rc::clone(shared);
    driver.set_sink(BodySink::Direct(Rc::new(move |event| {
        sink.defer(id, event)
    })));
    driver
}

fn declares_framing(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
    })
}

/// Resolves once the slot's head is committed, with the response Hyper will send.
struct SlotReady {
    slot: Rc<ResponseSlot>,
    id: u64,
}

impl Future for SlotReady {
    type Output = ResponseCommand;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.slot.inner.borrow_mut();
        if let Some((status, headers)) = inner.flushed.take() {
            let body = inner
                .body
                .take()
                .unwrap_or_else(|| {
                    let mut driver = ResponseBodyDriver::new(1);
                    driver.finish_without_body();
                    driver
                })
                .into_body();
            return Poll::Ready(ResponseCommand {
                id: self.id,
                status,
                headers,
                body,
            });
        }
        inner.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

impl HttpServerEngine {
    /// Spawn the connection future for `resource` over `transport`, dispatching to `dispatch`.
    pub fn attach(
        &mut self,
        resource: ResourceId,
        transport: HostTransport,
        allow_half_close: bool,
        dispatch: Rc<dyn ServerDispatch>,
    ) -> bool {
        if self.connections.borrow().contains_key(&resource) {
            return false;
        }
        let shared = Rc::new(ConnectionShared {
            resource,
            dispatch,
            requests: RefCell::new(HashMap::new()),
            deferred: RefCell::new(Vec::new()),
            deferred_wake: Notify::new(),
            shutdown: Notify::new(),
            closing: Cell::new(false),
            failed: Cell::new(false),
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

    pub fn start_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        high_water_mark: usize,
    ) -> bool {
        let Some(slot) = self.slot(resource, id) else {
            return false;
        };
        slot.start(status, headers, high_water_mark)
    }

    pub fn write_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.write(&shared, id, data)
    }

    pub fn end_response(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: Vec<u8>,
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.end(&shared, id, data)
    }

    /// Stop a connection. Graceful lets the response in flight finish; abort drops the
    /// transport and reports `Closed` on the next task turn, never inside the caller.
    pub fn close(&mut self, resource: ResourceId, mode: ServerCloseMode) -> bool {
        let table = self.connections.borrow();
        let Some(connection) = table.get(&resource) else {
            return false;
        };
        trace!("{resource:?} close {mode:?}");
        match mode {
            ServerCloseMode::Graceful => {
                connection.shared.closing.set(true);
                connection.shared.shutdown.notify_one();
            }
            ServerCloseMode::Abort => {
                connection.task.abort();
                let shared = Rc::clone(&connection.shared);
                let table = Rc::clone(&self.connections);
                drop(table.borrow());
                tokio::task::spawn_local(async move {
                    let _ = shared.dispatch(&ServerEvent::bare(ServerEventKind::Closed, 0));
                    table.borrow_mut().remove(&shared.resource);
                });
                return true;
            }
        }
        true
    }

    pub fn open_connections(&self) -> usize {
        self.connections.borrow().len()
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
    builder.half_close(allow_half_close);
    let mut connection = Box::pin(builder.serve_connection(TokioIo::new(transport.0), service));
    let pump = Rc::clone(&shared);
    let result = tokio::select! {
        biased;
        result = &mut connection => result,
        _ = async {
            loop {
                pump.deferred_wake.notified().await;
                if !pump.flush_deferred() { break; }
            }
        } => connection.await,
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
        let event = ServerEvent {
            message: message.as_bytes(),
            ..ServerEvent::bare(ServerEventKind::Error, 0)
        };
        let _ = shared.dispatch(&event);
    }
    let _ = shared.flush_deferred();
    let _ = shared.dispatch(&ServerEvent::bare(ServerEventKind::Closed, 0));
    table.borrow_mut().remove(&shared.resource);
}

async fn handle(
    shared: Rc<ConnectionShared>,
    id: u64,
    request: Request<Incoming>,
) -> Response<ResponseBody> {
    let (parts, mut body) = request.into_parts();
    let parsed = kiln_request_parts(&parts);
    let http_10 = parsed.version == HttpVersion::Http10;
    let head_request = parsed.method.eq_ignore_ascii_case("HEAD");
    let keep_alive = parsed.keep_alive;
    let slot = Rc::new(ResponseSlot::new(head_request));
    shared.requests.borrow_mut().insert(id, Rc::clone(&slot));

    let headers = flatten_header_map(&parts.headers);
    let head = ServerEvent {
        keep_alive,
        version_minor: u8::from(!http_10),
        method: parsed.method.as_bytes(),
        target: parsed.target.as_bytes(),
        headers: &headers,
        ..ServerEvent::bare(ServerEventKind::RequestHead, id)
    };
    if shared.dispatch(&head) {
        let mut failed = false;
        while let Some(frame) = body.frame().await {
            match frame {
                Ok(frame) => {
                    if let Ok(data) = frame.into_data()
                        && !data.is_empty()
                    {
                        let event = ServerEvent {
                            data: &data,
                            ..ServerEvent::bare(ServerEventKind::RequestData, id)
                        };
                        if !shared.dispatch(&event) {
                            failed = true;
                            break;
                        }
                    }
                }
                Err(error) => {
                    let message = error.to_string();
                    let event = ServerEvent {
                        message: message.as_bytes(),
                        ..ServerEvent::bare(ServerEventKind::Error, id)
                    };
                    let _ = shared.dispatch(&event);
                    failed = true;
                    break;
                }
            }
        }
        if !failed {
            let _ = shared.dispatch(&ServerEvent::bare(ServerEventKind::RequestEnd, id));
        }
    }
    let _ = shared.flush_deferred();

    let mut response = if shared.failed.get() {
        unavailable(id)
    } else {
        SlotReady {
            slot: Rc::clone(&slot),
            id,
        }
        .await
    };
    shared.requests.borrow_mut().remove(&id);
    let _ = shared.flush_deferred();

    let close_delimited = http_10
        && !head_request
        && response_status_allows_body(response.status)
        && !declares_content_length(&response.headers);
    let announces_connection = response
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("connection"));
    if !announces_connection && (!keep_alive || close_delimited || shared.closing.get()) {
        response.headers.push(("connection".into(), "close".into()));
    }
    hyper_response(response, close_delimited)
}

fn unavailable(id: u64) -> ResponseCommand {
    let mut driver = ResponseBodyDriver::new(1);
    driver.end(Vec::new());
    ResponseCommand {
        id,
        status: 503,
        headers: vec![("connection".into(), "close".into())],
        body: driver.into_body(),
    }
}

fn flatten_header_map(headers: &hyper::HeaderMap) -> Vec<u8> {
    let capacity = headers.iter().fold(0usize, |total, (name, value)| {
        total + name.as_str().len() + value.len() + 2
    });
    let mut flattened = Vec::with_capacity(capacity);
    for (name, value) in headers {
        flattened.extend_from_slice(name.as_str().as_bytes());
        flattened.push(0);
        flattened.extend_from_slice(value.as_bytes());
        flattened.push(0);
    }
    flattened
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Recording {
        events: RefCell<Vec<(ServerEventKind, u64, Vec<u8>)>>,
        checkpoints: Cell<usize>,
        answer: RefCell<Option<Rc<RefCell<Option<Box<dyn Fn(u64)>>>>>>,
    }

    impl ServerDispatch for Recording {
        fn event(&self, event: &ServerEvent<'_>) -> i32 {
            let payload = match event.kind {
                ServerEventKind::RequestHead => {
                    let mut bytes = event.method.to_vec();
                    bytes.push(b' ');
                    bytes.extend_from_slice(event.target);
                    bytes.push(b' ');
                    bytes.extend_from_slice(event.headers);
                    bytes
                }
                ServerEventKind::RequestData => event.data.to_vec(),
                ServerEventKind::Error => event.message.to_vec(),
                _ => Vec::new(),
            };
            self.events
                .borrow_mut()
                .push((event.kind, event.id, payload));
            if event.kind == ServerEventKind::RequestEnd
                && let Some(answer) = self.answer.borrow().as_ref()
                && let Some(answer) = answer.borrow().as_ref()
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

    #[test]
    fn a_request_is_dispatched_directly_and_answered_through_the_slot() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (client, server) = tokio::io::duplex(4096);
            let engine = Rc::new(RefCell::new(HttpServerEngine::default()));
            let recording = Rc::new(Recording {
                events: RefCell::new(Vec::new()),
                checkpoints: Cell::new(0),
                answer: RefCell::new(None),
            });
            let resource = ResourceId::new(7);
            let answer_engine = Rc::clone(&engine);
            let answer: Box<dyn Fn(u64)> = Box::new(move |id| {
                let mut engine = answer_engine.borrow_mut();
                assert!(engine.start_response(
                    resource,
                    id,
                    200,
                    vec![("content-type".into(), "text/plain".into())],
                    1024,
                ));
                let disposition = engine
                    .end_response(resource, id, b"kiln".to_vec())
                    .expect("response ends");
                assert!(disposition.ready);
            });
            *recording.answer.borrow_mut() = Some(Rc::new(RefCell::new(Some(answer))));
            assert!(engine.borrow_mut().attach(
                resource,
                HostTransport(Box::new(server)),
                false,
                recording.clone(),
            ));

            let mut client = client;
            client
                .write_all(b"GET /hello HTTP/1.1\r\nhost: x\r\nx-a: 1\r\n\r\n")
                .await
                .expect("request writes");
            let mut answer = Vec::new();
            let mut buffer = [0u8; 1024];
            let mut deadline = VecDeque::from([0u8; 0]);
            loop {
                let read = client.read(&mut buffer).await.expect("response reads");
                answer.extend_from_slice(&buffer[..read]);
                if answer.ends_with(b"kiln") {
                    break;
                }
                deadline.push_back(0);
                assert!(deadline.len() < 64, "response never completed: {answer:?}");
            }
            let text = String::from_utf8(answer).expect("utf-8 response");
            assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
            assert!(text.contains("content-length: 4\r\n"), "{text}");
            assert!(!text.contains("transfer-encoding"), "{text}");

            drop(client);
            tokio::task::yield_now().await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let events = recording.events.borrow();
            let kinds: Vec<_> = events.iter().map(|(kind, _, _)| *kind).collect();
            assert_eq!(
                kinds,
                vec![
                    ServerEventKind::RequestHead,
                    ServerEventKind::RequestEnd,
                    ServerEventKind::Finish,
                    ServerEventKind::Closed,
                ],
                "{events:?}"
            );
            assert_eq!(
                events[0].2,
                b"GET /hello host\0x\0x-a\x001\0".to_vec(),
                "head carries method, target and NUL-flattened headers"
            );
            assert_eq!(recording.checkpoints.get(), events.len());
            assert_eq!(engine.borrow().open_connections(), 0);
        });
    }
}

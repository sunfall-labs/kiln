//! HTTP client connections as Hyper tasks over host transports (ADR 0046).
//!
//! A connection is one task that performs Hyper's HTTP/1 handshake over the transport the host
//! handed over and then sends the requests the language starts, in order. Response head, chunk
//! and end call the realm's adapter directly, on the realm thread, inside the task's poll; the
//! request body the language is still writing goes through the same body driver a server
//! response uses, with its `drain` and `finish` deferred to the task's next safe point.
//!
//! The end of a response carries the Agent's reuse decision, as the bridged engine's did: the
//! adapter must not have to wait for a second event to learn whether the socket can take the
//! next request, and a `Connection: close` response still observes `end` before `Closed`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::Request;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper_util::rt::TokioIo;
use kiln_host_abi::ResourceId;
use tokio::sync::Notify;

use crate::HttpCloseMode;
use crate::http_body::{BodyEvent, ResponseBodyDriver};
use crate::http_engine::{
    Dispatcher, EngineBody, HashMap, HostTransport, HttpDispatch, HttpEvent, HttpEventKind,
    UpgradeTransport, flatten_header_map, trace,
};
use crate::node_stream::WriteDisposition;

/// The table of live client connections. Lives in the realm state; tasks reach it through
/// `Rc` so a finished connection can retire itself.
#[derive(Default)]
pub struct HttpClientEngine {
    connections: Rc<RefCell<HashMap<ResourceId, ClientConnection>>>,
    upgraded: Rc<RefCell<HashMap<ResourceId, UpgradeTransport>>>,
}

struct ClientConnection {
    shared: Rc<ClientShared>,
    task: tokio::task::JoinHandle<()>,
}

struct ClientShared {
    dispatcher: Rc<Dispatcher>,
    /// Requests the language has started, in order, that Hyper has not been handed yet.
    queue: RefCell<VecDeque<QueuedRequest>>,
    queue_wake: Notify,
    /// Bodies the language may still be writing, by request.
    slots: RefCell<HashMap<u64, Rc<RequestSlot>>>,
    /// Requests started by the language whose `ResponseEnd` or `Error` has not been dispatched.
    in_flight: Cell<usize>,
    shutdown: Notify,
    closing: Cell<bool>,
    /// An abort has been requested and its `Closed` has not been published yet.
    abort_pending: Cell<bool>,
    /// Hyper's connection future runs as its own task; an abort has to reach it too.
    connection: RefCell<Option<tokio::task::AbortHandle>>,
    upgrade_wake: Notify,
    upgrade_parts: RefCell<Option<(HostTransport, Vec<u8>)>>,
    upgrade_error: RefCell<Option<String>>,
    header_scratch: RefCell<Vec<u8>>,
}

struct QueuedRequest {
    id: u64,
    method: hyper::Method,
    target: String,
    headers: HeaderMap,
    slot: Rc<RequestSlot>,
}

/// Where a request body accumulates until the task hands the request to Hyper.
pub struct RequestSlot {
    inner: RefCell<SlotInner>,
}

struct SlotInner {
    body: SlotBody,
    high_water_mark: usize,
}

enum SlotBody {
    /// Nothing written yet. `end(data)` makes this a complete body; `write` makes it a stream.
    Unstarted,
    Complete(Vec<u8>),
    Streaming(ResponseBodyDriver),
    /// The request went out without a body (a `GET` nothing was written to before the task sent
    /// it). Later writes are accepted and dropped, as Node does for a request already sent.
    Discarded,
}

impl RequestSlot {
    fn new(high_water_mark: usize) -> Self {
        Self {
            inner: RefCell::new(SlotInner {
                body: SlotBody::Unstarted,
                high_water_mark: high_water_mark.max(1),
            }),
        }
    }

    fn write(&self, dispatcher: &Rc<Dispatcher>, id: u64, data: &[u8]) -> Option<WriteDisposition> {
        let mut inner = self.inner.borrow_mut();
        let high_water_mark = inner.high_water_mark;
        match &mut inner.body {
            SlotBody::Streaming(driver) => Some(driver.write(data.to_vec())),
            SlotBody::Unstarted => {
                let mut driver = dispatcher.body_driver(id, high_water_mark);
                let disposition = driver.write(data.to_vec());
                inner.body = SlotBody::Streaming(driver);
                Some(disposition)
            }
            SlotBody::Discarded => Some(WriteDisposition {
                ready: true,
                submit_now: false,
            }),
            SlotBody::Complete(_) => None,
        }
    }

    fn end(&self, dispatcher: &Rc<Dispatcher>, id: u64, data: &[u8]) -> Option<WriteDisposition> {
        let mut inner = self.inner.borrow_mut();
        match &mut inner.body {
            SlotBody::Streaming(driver) => Some(driver.end(data.to_vec())),
            SlotBody::Unstarted => {
                // The whole body arrived with `end`: Hyper gets one frame and declares its
                // length, and `finish` follows as soon as the language's call has returned.
                inner.body = SlotBody::Complete(data.to_vec());
                dispatcher.defer(id, BodyEvent::Finish);
                Some(WriteDisposition {
                    ready: true,
                    submit_now: false,
                })
            }
            SlotBody::Discarded => {
                dispatcher.defer(id, BodyEvent::Finish);
                Some(WriteDisposition {
                    ready: true,
                    submit_now: false,
                })
            }
            SlotBody::Complete(_) => None,
        }
    }

    /// The body Hyper sends, decided when the task takes the request. A body that is complete
    /// by then declares its length; one still being written is chunked; a method that carries
    /// no body by default and has had nothing written sends none, and the slot is discarded.
    fn take_body(
        &self,
        dispatcher: &Rc<Dispatcher>,
        id: u64,
        method: &hyper::Method,
        headers: &mut HeaderMap,
    ) -> EngineBody {
        let mut inner = self.inner.borrow_mut();
        let high_water_mark = inner.high_water_mark;
        let body_by_default = !matches!(
            *method,
            hyper::Method::GET
                | hyper::Method::HEAD
                | hyper::Method::DELETE
                | hyper::Method::OPTIONS
                | hyper::Method::TRACE
                | hyper::Method::CONNECT
        );
        match std::mem::replace(&mut inner.body, SlotBody::Discarded) {
            SlotBody::Complete(bytes) if bytes.is_empty() => {
                // Node writes `Content-Length: 0` for a bodiless POST; nothing for a GET.
                if body_by_default
                    && !headers.contains_key(hyper::header::CONTENT_LENGTH)
                    && !headers.contains_key(hyper::header::TRANSFER_ENCODING)
                {
                    headers.insert(hyper::header::CONTENT_LENGTH, HeaderValue::from_static("0"));
                }
                EngineBody::Empty
            }
            SlotBody::Complete(bytes) => EngineBody::Complete(Some(Bytes::from(bytes))),
            SlotBody::Streaming(driver) => {
                let body = driver.into_body();
                inner.body = SlotBody::Streaming(driver);
                EngineBody::Streaming(body)
            }
            SlotBody::Unstarted if body_by_default => {
                let driver = dispatcher.body_driver(id, high_water_mark);
                let body = driver.into_body();
                inner.body = SlotBody::Streaming(driver);
                EngineBody::Streaming(body)
            }
            SlotBody::Unstarted | SlotBody::Discarded => EngineBody::Empty,
        }
    }
}

impl ClientShared {
    async fn next_request(&self) -> QueuedRequest {
        loop {
            if let Some(request) = self.queue.borrow_mut().pop_front() {
                return request;
            }
            self.queue_wake.notified().await;
        }
    }

    fn request_done(&self, id: u64) {
        self.slots.borrow_mut().remove(&id);
        self.in_flight.set(self.in_flight.get().saturating_sub(1));
    }

    async fn take_upgrade_parts(&self) -> Result<(HostTransport, Vec<u8>), String> {
        loop {
            let notified = self.upgrade_wake.notified();
            if let Some(parts) = self.upgrade_parts.borrow_mut().take() {
                return Ok(parts);
            }
            if let Some(error) = self.upgrade_error.borrow_mut().take() {
                return Err(error);
            }
            notified.await;
        }
    }

    /// Fail every request the language started that Hyper never answered, queued or in
    /// progress. Called once the connection is gone, before `Closed`.
    fn fail_outstanding(&self, message: &str) {
        self.queue.borrow_mut().clear();
        let mut outstanding: Vec<u64> = self.slots.borrow().keys().copied().collect();
        outstanding.sort_unstable();
        for id in outstanding {
            self.request_done(id);
            let event = HttpEvent {
                message: message.as_bytes(),
                ..HttpEvent::bare(HttpEventKind::Error, id)
            };
            let _ = self.dispatcher.dispatch(&event);
        }
    }
}

impl HttpClientEngine {
    pub fn take_upgrade(&mut self, resource: ResourceId) -> Option<UpgradeTransport> {
        self.upgraded.borrow_mut().remove(&resource)
    }
    /// Spawn the connection task for `resource` over `transport`, dispatching to `dispatch`.
    pub fn attach(
        &mut self,
        resource: ResourceId,
        transport: HostTransport,
        dispatch: Rc<dyn HttpDispatch>,
    ) -> bool {
        if self.connections.borrow().contains_key(&resource) {
            return false;
        }
        let shared = Rc::new(ClientShared {
            dispatcher: Rc::new(Dispatcher::new(resource, dispatch)),
            queue: RefCell::new(VecDeque::new()),
            queue_wake: Notify::new(),
            slots: RefCell::new(HashMap::default()),
            in_flight: Cell::new(0),
            shutdown: Notify::new(),
            closing: Cell::new(false),
            abort_pending: Cell::new(false),
            connection: RefCell::new(None),
            upgrade_wake: Notify::new(),
            upgrade_parts: RefCell::new(None),
            upgrade_error: RefCell::new(None),
            header_scratch: RefCell::new(Vec::with_capacity(512)),
        });
        trace!("{resource:?} client attach");
        let table = Rc::clone(&self.connections);
        let upgraded = Rc::clone(&self.upgraded);
        let task = tokio::task::spawn_local(run(transport, Rc::clone(&shared), table, upgraded));
        self.connections
            .borrow_mut()
            .insert(resource, ClientConnection { shared, task });
        true
    }

    pub fn is_attached(&self, resource: ResourceId) -> bool {
        self.connections.borrow().contains_key(&resource)
    }

    /// Start one request on an attached connection. The head is sent when the task takes it;
    /// the body is whatever `write_request` and `end_request` have put in the slot by then.
    /// An invalid method, target or header refuses the request.
    pub fn send_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        method: &str,
        target: &str,
        headers: &[(String, String)],
        high_water_mark: usize,
    ) -> bool {
        let shared = {
            let table = self.connections.borrow();
            let Some(connection) = table.get(&resource) else {
                return false;
            };
            Rc::clone(&connection.shared)
        };
        if shared.slots.borrow().contains_key(&id) || shared.closing.get() {
            return false;
        }
        let Ok(method) = hyper::Method::from_bytes(method.as_bytes()) else {
            return false;
        };
        if target.parse::<hyper::Uri>().is_err() {
            return false;
        }
        let mut header_map = HeaderMap::with_capacity(headers.len());
        for (name, value) in headers {
            let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) else {
                return false;
            };
            header_map.append(name, value);
        }
        let slot = Rc::new(RequestSlot::new(high_water_mark));
        shared.slots.borrow_mut().insert(id, Rc::clone(&slot));
        shared.in_flight.set(shared.in_flight.get() + 1);
        shared.queue.borrow_mut().push_back(QueuedRequest {
            id,
            method,
            target: target.to_owned(),
            headers: header_map,
            slot,
        });
        shared.queue_wake.notify_one();
        trace!("{resource:?} client request id={id} queued");
        true
    }

    pub fn write_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.write(&shared.dispatcher, id, data)
    }

    pub fn end_request(
        &mut self,
        resource: ResourceId,
        id: u64,
        data: &[u8],
    ) -> Option<WriteDisposition> {
        let (shared, slot) = self.shared_and_slot(resource, id)?;
        slot.end(&shared.dispatcher, id, data)
    }

    /// Stop a connection. Graceful lets the request in flight finish; abort drops the
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
        trace!("{resource:?} client close {mode:?}");
        match mode {
            HttpCloseMode::Graceful => {
                shared.closing.set(true);
                shared.shutdown.notify_one();
            }
            HttpCloseMode::Abort => {
                if shared.abort_pending.replace(true) {
                    return true;
                }
                if let Some(connection) = shared.connection.borrow_mut().take() {
                    connection.abort();
                }
                let table = Rc::clone(&self.connections);
                tokio::task::spawn_local(async move {
                    shared.fail_outstanding("socket hang up");
                    let _ = shared
                        .dispatcher
                        .dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
                    shared.abort_pending.set(false);
                    table.borrow_mut().remove(&shared.dispatcher.resource);
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
            if let Some(handle) = connection.shared.connection.borrow_mut().take() {
                handle.abort();
            }
            connection.task.abort();
        }
    }

    /// Requests the language started whose end has not been dispatched: what keeps a drive
    /// waiting when no host operation is referenced, since a taken transport is not.
    pub fn requests_in_flight(&self) -> usize {
        self.connections
            .borrow()
            .values()
            .map(|connection| {
                connection.shared.in_flight.get()
                    + usize::from(connection.shared.abort_pending.get())
            })
            .sum()
    }

    fn shared_and_slot(
        &self,
        resource: ResourceId,
        id: u64,
    ) -> Option<(Rc<ClientShared>, Rc<RequestSlot>)> {
        let table = self.connections.borrow();
        let connection = table.get(&resource)?;
        let slot = connection.shared.slots.borrow().get(&id).cloned()?;
        Some((Rc::clone(&connection.shared), slot))
    }
}

/// Why a request could not be answered.
enum Failure {
    /// A protocol or transport error, reported against the request.
    Error(String),
    /// The connection is finished; the request is already reported.
    Closed,
}

async fn run(
    transport: HostTransport,
    shared: Rc<ClientShared>,
    table: Rc<RefCell<HashMap<ResourceId, ClientConnection>>>,
    upgraded: Rc<RefCell<HashMap<ResourceId, UpgradeTransport>>>,
) {
    shared.dispatcher.started();
    let dispatcher = Rc::clone(&shared.dispatcher);
    let handshake = hyper::client::conn::http1::handshake(TokioIo::new(transport.0));
    let (mut sender, connection) = match handshake.await {
        Ok(parts) => parts,
        Err(error) => {
            let message = error.to_string();
            let _ = dispatcher.dispatch(&HttpEvent {
                message: message.as_bytes(),
                ..HttpEvent::bare(HttpEventKind::Error, 0)
            });
            finish(&shared, &table, false);
            return;
        }
    };
    let connection_shared = Rc::clone(&shared);
    let connection = tokio::task::spawn_local(async move {
        match connection.without_shutdown().await {
            Ok(parts) => {
                let raw = parts.io.into_inner();
                *connection_shared.upgrade_parts.borrow_mut() =
                    Some((HostTransport(raw), parts.read_buf.to_vec()));
                connection_shared.upgrade_wake.notify_one();
                Ok(())
            }
            Err(error) => {
                *connection_shared.upgrade_error.borrow_mut() = Some(error.to_string());
                connection_shared.upgrade_wake.notify_one();
                Err(error)
            }
        }
    });
    *shared.connection.borrow_mut() = Some(connection.abort_handle());
    let mut connection = Some(connection);
    let mut transferred = false;
    loop {
        let request = tokio::select! {
            biased;
            result = async {
                match connection.as_mut() {
                    Some(connection) => connection.await,
                    None => std::future::pending().await,
                }
            } => {
                // The peer closed, or the transport failed, with no request in flight. That is
                // not a request failure: an idle keep-alive socket reset by its server is a
                // close and nothing more.
                trace!("{:?} client connection ended idle: {:?}", dispatcher.resource, result.err());
                connection = None;
                break;
            }
            _ = shared.shutdown.notified() => break,
            _ = dispatcher.pump() => break,
            request = shared.next_request() => request,
        };
        let QueuedRequest {
            id,
            method,
            target,
            mut headers,
            slot,
        } = request;
        let body = slot.take_body(&dispatcher, id, &method, &mut headers);
        let mut builder = Request::builder().method(method).uri(target);
        if let Some(map) = builder.headers_mut() {
            *map = headers;
        }
        let request = match builder.body(body) {
            Ok(request) => request,
            Err(error) => {
                shared.request_done(id);
                let message = error.to_string();
                let _ = dispatcher.dispatch(&HttpEvent {
                    message: message.as_bytes(),
                    ..HttpEvent::bare(HttpEventKind::Error, id)
                });
                continue;
            }
        };
        let outcome = tokio::select! {
            biased;
            outcome = exchange(&shared, &mut sender, id, request, &upgraded) => outcome,
            _ = dispatcher.pump() => Err(Failure::Closed),
        };
        match outcome {
            Ok(ExchangeOutcome::Reusable) => {
                if shared.closing.get() {
                    break;
                }
            }
            Ok(ExchangeOutcome::Retired) => break,
            Ok(ExchangeOutcome::Transferred) => {
                transferred = true;
                break;
            }
            Err(Failure::Error(message)) => {
                shared.request_done(id);
                let _ = dispatcher.dispatch(&HttpEvent {
                    message: message.as_bytes(),
                    ..HttpEvent::bare(HttpEventKind::Error, id)
                });
                break;
            }
            Err(Failure::Closed) => break,
        }
    }
    drop(sender);
    if let Some(connection) = connection {
        connection.abort();
    }
    shared.connection.borrow_mut().take();
    shared.fail_outstanding("socket hang up");
    finish(&shared, &table, transferred);
}

enum ExchangeOutcome {
    Reusable,
    Retired,
    Transferred,
}

/// Send one request and deliver its response. `Ok(reusable)` once the response has ended.
async fn exchange(
    shared: &Rc<ClientShared>,
    sender: &mut hyper::client::conn::http1::SendRequest<EngineBody>,
    id: u64,
    request: Request<EngineBody>,
    upgraded: &Rc<RefCell<HashMap<ResourceId, UpgradeTransport>>>,
) -> Result<ExchangeOutcome, Failure> {
    let dispatcher = &shared.dispatcher;
    if let Err(error) = sender.ready().await {
        return Err(Failure::Error(error.to_string()));
    }
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| Failure::Error(error.to_string()))?;
    if response.status() == hyper::StatusCode::SWITCHING_PROTOCOLS {
        let (response_parts, _body) = response.into_parts();
        let reason = response_parts.status.canonical_reason().unwrap_or_default();
        let (stream, head) = shared.take_upgrade_parts().await.map_err(Failure::Error)?;
        let mut scratch = shared.header_scratch.borrow_mut();
        flatten_header_map(&response_parts.headers, &mut scratch);
        upgraded.borrow_mut().insert(
            shared.dispatcher.resource,
            UpgradeTransport {
                transport: stream,
                head: head.clone(),
                pending_write: Vec::new(),
                write_fences: Vec::new(),
                write_completion: crate::http_engine::UpgradeWriteCompletion::completed(),
            },
        );
        let event = HttpEvent {
            status: response_parts.status.as_u16(),
            reason: reason.as_bytes(),
            headers: &scratch,
            data: &head,
            ..HttpEvent::bare(HttpEventKind::UpgradeResponse, id)
        };
        let dispatched = dispatcher.dispatch(&event);
        drop(scratch);
        if !dispatched {
            if let Some(transfer) = upgraded.borrow_mut().remove(&shared.dispatcher.resource) {
                transfer.write_completion.complete(Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "HTTP upgrade dispatch failed before transport adoption",
                )));
                return Err(Failure::Closed);
            }
            shared.request_done(id);
            return Ok(ExchangeOutcome::Transferred);
        }
        shared.request_done(id);
        return Ok(ExchangeOutcome::Transferred);
    }
    let (parts, mut body) = response.into_parts();
    let reason = parts.status.canonical_reason().unwrap_or_default();
    {
        let mut scratch = shared.header_scratch.borrow_mut();
        flatten_header_map(&parts.headers, &mut scratch);
    }
    let dispatched = {
        let scratch = shared.header_scratch.borrow();
        let head = HttpEvent {
            status: parts.status.as_u16(),
            reason: reason.as_bytes(),
            headers: &scratch,
            ..HttpEvent::bare(HttpEventKind::ResponseHead, id)
        };
        dispatcher.dispatch(&head)
    };
    if !dispatched {
        return Err(Failure::Closed);
    }
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(frame) => {
                if let Ok(data) = frame.into_data()
                    && !data.is_empty()
                {
                    let event = HttpEvent {
                        data: &data,
                        ..HttpEvent::bare(HttpEventKind::ResponseData, id)
                    };
                    if !dispatcher.dispatch(&event) {
                        return Err(Failure::Closed);
                    }
                }
            }
            Err(error) => return Err(Failure::Error(error.to_string())),
        }
    }
    // `ResponseEnd` is both the Node-visible terminal body event and the Agent's reuse
    // decision. Carry the decision in the same event so adapters cannot block between two
    // events; a `Connection: close` response still observes `end` before `Closed`.
    let reusable = sender.ready().await.is_ok() && !shared.closing.get();
    shared.request_done(id);
    let event = HttpEvent {
        keep_alive: reusable,
        ..HttpEvent::bare(HttpEventKind::ResponseEnd, id)
    };
    if !dispatcher.dispatch(&event) {
        return Err(Failure::Closed);
    }
    Ok(if reusable {
        ExchangeOutcome::Reusable
    } else {
        ExchangeOutcome::Retired
    })
}

fn finish(
    shared: &Rc<ClientShared>,
    table: &Rc<RefCell<HashMap<ResourceId, ClientConnection>>>,
    transferred: bool,
) {
    let _ = shared.dispatcher.flush_deferred();
    if !transferred {
        // `without_shutdown` also returns the IO when an ordinary connection closes. Keep its
        // parts only while a 101 exchange can claim them; otherwise dropping them retires IO.
        shared.upgrade_parts.borrow_mut().take();
        let _ = shared
            .dispatcher
            .dispatch(&HttpEvent::bare(HttpEventKind::Closed, 0));
    }
    table.borrow_mut().remove(&shared.dispatcher.resource);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Recording {
        events: RefCell<Vec<(HttpEventKind, u64, bool, Vec<u8>)>>,
        checkpoints: Cell<usize>,
    }

    impl HttpDispatch for Recording {
        fn event(&self, event: &HttpEvent<'_>) -> i32 {
            let payload = match event.kind {
                HttpEventKind::ResponseHead => {
                    let mut bytes = event.status.to_string().into_bytes();
                    bytes.push(b' ');
                    bytes.extend_from_slice(event.reason);
                    bytes.push(b' ');
                    bytes.extend_from_slice(event.headers);
                    bytes
                }
                HttpEventKind::ResponseData => event.data.to_vec(),
                HttpEventKind::Error => event.message.to_vec(),
                _ => Vec::new(),
            };
            self.events
                .borrow_mut()
                .push((event.kind, event.id, event.keep_alive, payload));
            0
        }

        fn checkpoint(&self) -> i32 {
            self.checkpoints.set(self.checkpoints.get() + 1);
            0
        }
    }

    fn recording() -> Rc<Recording> {
        Rc::new(Recording {
            events: RefCell::new(Vec::new()),
            checkpoints: Cell::new(0),
        })
    }

    fn local() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime builds");
        (runtime, tokio::task::LocalSet::new())
    }

    async fn read_request(server: &mut tokio::io::DuplexStream, ends_with: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 1024];
        for _ in 0..64 {
            let read = server.read(&mut buffer).await.expect("request reads");
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.ends_with(ends_with) || read == 0 {
                break;
            }
        }
        bytes
    }

    fn kinds(recording: &Recording) -> Vec<HttpEventKind> {
        recording
            .events
            .borrow()
            .iter()
            .map(|(kind, _, _, _)| *kind)
            .collect()
    }

    #[test]
    fn a_complete_body_declares_its_length_and_the_response_is_dispatched_directly() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (client, mut server) = tokio::io::duplex(4096);
            let mut engine = HttpClientEngine::default();
            let recording = recording();
            let resource = ResourceId::new(5);
            assert!(engine.attach(resource, HostTransport(Box::new(client)), recording.clone()));
            assert!(engine.send_request(
                resource,
                1,
                "POST",
                "/upload",
                &[("host".into(), "kiln".into())],
                1024,
            ));
            assert!(engine.end_request(resource, 1, b"kiln").expect("ends").ready);
            assert_eq!(engine.requests_in_flight(), 1);

            let request = read_request(&mut server, b"kiln").await;
            let text = String::from_utf8(request).expect("ASCII request");
            assert!(text.starts_with("POST /upload HTTP/1.1\r\n"), "{text}");
            assert!(text.contains("host: kiln\r\n"), "{text}");
            assert!(text.contains("content-length: 4\r\n"), "{text}");
            assert!(!text.contains("transfer-encoding"), "{text}");

            server
                .write_all(b"HTTP/1.1 201 Created\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\nok")
                .await
                .expect("response writes");
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            {
                let events = recording.events.borrow();
                let dispatched: Vec<_> = events.iter().map(|(kind, _, _, _)| *kind).collect();
                assert_eq!(
                    dispatched,
                    vec![
                        HttpEventKind::Finish,
                        HttpEventKind::ResponseHead,
                        HttpEventKind::ResponseData,
                        HttpEventKind::ResponseEnd,
                    ],
                    "{events:?}"
                );
                assert_eq!(
                    events[1].3,
                    b"201 Created content-type\0text/plain\0content-length\x002\0".to_vec()
                );
                assert_eq!(events[2].3, b"ok".to_vec());
                assert!(events[3].2, "a keep-alive response leaves the socket reusable");
                assert_eq!(recording.checkpoints.get(), events.len());
            }
            assert_eq!(engine.requests_in_flight(), 0);
            assert_eq!(engine.open_connections(), 1, "the connection idles for reuse");

            // The peer closes the idle socket: `Closed` alone, no error.
            drop(server);
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert_eq!(kinds(&recording).last(), Some(&HttpEventKind::Closed));
            assert!(!kinds(&recording).contains(&HttpEventKind::Error));
            assert_eq!(engine.open_connections(), 0);
        });
    }

    #[test]
    fn a_streamed_body_is_chunked_and_sequential_requests_reuse_the_socket() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (client, mut server) = tokio::io::duplex(8192);
            let mut engine = HttpClientEngine::default();
            let recording = recording();
            let resource = ResourceId::new(6);
            assert!(engine.attach(resource, HostTransport(Box::new(client)), recording.clone()));
            assert!(engine.send_request(resource, 1, "POST", "/stream", &[], 8));
            assert!(
                engine
                    .write_request(resource, 1, b"ab")
                    .expect("writes")
                    .ready
            );
            let head = read_request(&mut server, b"2\r\nab\r\n").await;
            let text = String::from_utf8(head).expect("ASCII request");
            assert!(text.contains("transfer-encoding: chunked\r\n"), "{text}");
            assert!(text.ends_with("2\r\nab\r\n"), "{text}");
            engine.end_request(resource, 1, b"cd").expect("ends");
            let tail = read_request(&mut server, b"0\r\n\r\n").await;
            assert_eq!(tail, b"2\r\ncd\r\n0\r\n\r\n".to_vec());
            server
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1\r\n\r\n1")
                .await
                .expect("first response writes");
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            assert!(kinds(&recording).contains(&HttpEventKind::ResponseEnd));

            // A second request on the same connection; a bodiless GET sends no body headers.
            assert!(engine.send_request(resource, 2, "GET", "/second", &[], 8));
            engine.end_request(resource, 2, b"").expect("ends");
            let request = read_request(&mut server, b"\r\n\r\n").await;
            let text = String::from_utf8(request).expect("ASCII request");
            assert!(text.starts_with("GET /second HTTP/1.1\r\n"), "{text}");
            assert!(!text.contains("content-length"), "{text}");
            assert!(!text.contains("transfer-encoding"), "{text}");
            server
                .write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 1\r\n\r\n2")
                .await
                .expect("second response writes");
            for _ in 0..16 {
                tokio::task::yield_now().await;
            }
            let events = recording.events.borrow();
            let ends: Vec<_> = events
                .iter()
                .filter(|(kind, _, _, _)| *kind == HttpEventKind::ResponseEnd)
                .map(|(_, id, reusable, _)| (*id, *reusable))
                .collect();
            assert_eq!(ends, vec![(1, true), (2, false)]);
            let last = events.last().map(|(kind, _, _, _)| *kind);
            assert_eq!(last, Some(HttpEventKind::Closed), "{events:?}");
            let closed_at = events
                .iter()
                .position(|(kind, _, _, _)| *kind == HttpEventKind::Closed)
                .expect("closed");
            let end_at = events
                .iter()
                .rposition(|(kind, _, _, _)| *kind == HttpEventKind::ResponseEnd)
                .expect("end");
            assert!(end_at < closed_at, "end precedes close: {events:?}");
            drop(events);
            assert_eq!(engine.open_connections(), 0);
        });
    }

    #[test]
    fn an_abort_fails_the_outstanding_request_and_publishes_closed_from_a_task() {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let (client, _server) = tokio::io::duplex(4096);
            let mut engine = HttpClientEngine::default();
            let recording = recording();
            let resource = ResourceId::new(8);
            assert!(engine.attach(resource, HostTransport(Box::new(client)), recording.clone()));
            assert!(engine.send_request(resource, 1, "GET", "/", &[], 8));
            engine.end_request(resource, 1, b"").expect("ends");
            tokio::task::yield_now().await;
            assert!(engine.close(resource, HttpCloseMode::Abort));
            assert!(
                !kinds(&recording).contains(&HttpEventKind::Closed),
                "closed is never published inside the caller"
            );
            assert_eq!(
                engine.requests_in_flight(),
                2,
                "the request stays in flight, and the abort counts until it is heard"
            );
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            let kinds = kinds(&recording);
            assert_eq!(
                kinds,
                vec![
                    HttpEventKind::Finish,
                    HttpEventKind::Error,
                    HttpEventKind::Closed
                ],
                "the outstanding request fails before the socket is reported closed"
            );
            assert_eq!(engine.requests_in_flight(), 0);
            assert_eq!(engine.open_connections(), 0);
        });
    }
}

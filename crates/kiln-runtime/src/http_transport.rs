//! Runtime-private byte transport between Hyper and the realm completion loop.
//!
//! Hyper needs a continuously-polled `AsyncRead + AsyncWrite`, while HostOps deliberately exposes
//! discrete, correlated reads and writes. This bounded channel joins those models without letting
//! Tokio or Hyper types cross the public runtime seam. The realm half feeds completed reads and
//! acknowledges completed writes; the Hyper half observes ordinary async I/O and backpressure.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use kiln_host_abi::{AsyncHostError, AsyncHostOp, Completion, RequestId, ResourceId};

use crate::node_stream::{DEFAULT_HIGH_WATER_MARK, StreamAction, StreamDriver, StreamEvent};

const DEFAULT_BUFFER_LIMIT: usize = 64 * 1024;

/// A portable resource after ownership has transferred to the realm HTTP engine.
///
/// Keeping this distinct from [`ResourceId`] prevents HTTP transport code from selecting raw
/// resource operations accidentally. Converting back to an untyped resource is explicit at a
/// HostOps adapter boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HttpResourceId(ResourceId);

impl HttpResourceId {
    /// Mark an accepted or connected TCP resource as owned by the HTTP engine.
    pub const fn new(resource: ResourceId) -> Self {
        Self(resource)
    }

    /// Return the provider resource identity at an explicit adapter boundary.
    pub const fn resource(self) -> ResourceId {
        self.0
    }
}

/// Host operation an HTTP transport may select directly.
///
/// There is deliberately no raw close variant. Closing an HTTP-owned resource is represented by
/// [`HttpStreamAction::Close`] and must pass through the runtime's authorized close adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpHostOp {
    TcpRead {
        stream: HttpResourceId,
        max_bytes: usize,
    },
    TcpWrite {
        stream: HttpResourceId,
        data: Vec<u8>,
    },
    TcpShutdown {
        stream: HttpResourceId,
    },
}

impl HttpHostOp {
    /// Lower one HTTP-safe operation at the provider boundary.
    pub fn into_host_op(self) -> AsyncHostOp {
        match self {
            Self::TcpRead { stream, max_bytes } => AsyncHostOp::TcpRead {
                stream: stream.resource(),
                max_bytes,
            },
            Self::TcpWrite { stream, data } => AsyncHostOp::TcpWrite {
                stream: stream.resource(),
                data,
            },
            Self::TcpShutdown { stream } => AsyncHostOp::TcpShutdown {
                stream: stream.resource(),
            },
        }
    }
}

/// One close selected by the runtime-owned HTTP transport state machine.
///
/// The constructor is deliberately private. Language adapters may inspect and consume a close
/// selected by the transport, but cannot fabricate one from a raw [`ResourceId`]. Keeping the
/// capability non-`Clone` also prevents one state-machine transition from authorizing two host
/// closes.
///
/// ```compile_fail
/// use kiln_host_abi::ResourceId;
/// use kiln_runtime::{HttpCloseCapability, HttpResourceId, HttpStreamAction};
///
/// let resource = HttpResourceId::new(ResourceId::new(7));
/// let close = HttpCloseCapability::new(resource);
/// let _ = HttpStreamAction::Close(close);
/// ```
///
/// ```compile_fail
/// use kiln_runtime::HttpCloseCapability;
///
/// fn duplicate(close: &HttpCloseCapability) -> HttpCloseCapability {
///     HttpCloseCapability::clone(close)
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct HttpCloseCapability {
    resource: HttpResourceId,
}

impl HttpCloseCapability {
    fn new(resource: HttpResourceId) -> Self {
        Self { resource }
    }

    /// Return the provider resource identity for adapter-side correlation checks.
    pub const fn resource(&self) -> ResourceId {
        self.resource.resource()
    }

    pub(crate) const fn into_resource(self) -> HttpResourceId {
        self.resource
    }
}

/// Side effect selected by an HTTP transport.
///
/// Unlike the generic Node stream action, this type cannot contain
/// `AsyncHostOp::CloseResource`. That makes bypassing HTTP's drain/abort state machine a type
/// error in interpreter and native adapters.
///
/// ```compile_fail
/// use kiln_host_abi::ResourceId;
/// use kiln_runtime::{HttpHostOp, HttpResourceId};
///
/// let resource = HttpResourceId::new(ResourceId::new(7));
/// let _ = HttpHostOp::CloseResource { resource };
/// ```
#[derive(Debug, PartialEq, Eq)]
pub enum HttpStreamAction {
    Start(HttpHostOp),
    Cancel(RequestId),
    Data(Vec<u8>),
    Event(StreamEvent),
    Error(AsyncHostError),
    Close(HttpCloseCapability),
}

fn http_action(action: StreamAction) -> HttpStreamAction {
    match action {
        StreamAction::Start(AsyncHostOp::TcpRead { stream, max_bytes }) => {
            HttpStreamAction::Start(HttpHostOp::TcpRead {
                stream: HttpResourceId::new(stream),
                max_bytes,
            })
        }
        StreamAction::Start(AsyncHostOp::TcpWrite { stream, data }) => {
            HttpStreamAction::Start(HttpHostOp::TcpWrite {
                stream: HttpResourceId::new(stream),
                data,
            })
        }
        StreamAction::Start(AsyncHostOp::TcpShutdown { stream }) => {
            HttpStreamAction::Start(HttpHostOp::TcpShutdown {
                stream: HttpResourceId::new(stream),
            })
        }
        StreamAction::Start(AsyncHostOp::CloseResource { resource }) => {
            HttpStreamAction::Close(HttpCloseCapability::new(HttpResourceId::new(resource)))
        }
        StreamAction::Cancel(request) => HttpStreamAction::Cancel(request),
        StreamAction::Data(data) => HttpStreamAction::Data(data),
        StreamAction::Event(event) => HttpStreamAction::Event(event),
        StreamAction::Error(error) => HttpStreamAction::Error(error),
        StreamAction::Start(operation) => {
            unreachable!("HTTP stream selected unsupported host operation {operation:?}")
        }
    }
}

fn http_actions(actions: Vec<StreamAction>) -> Vec<HttpStreamAction> {
    actions.into_iter().map(http_action).collect()
}

/// Build the two private halves of one HTTP connection transport.
pub(crate) fn channel() -> (HyperTransport, RealmTransport) {
    channel_with_limit(DEFAULT_BUFFER_LIMIT)
}

fn channel_with_limit(limit: usize) -> (HyperTransport, RealmTransport) {
    assert!(limit > 0, "HTTP transport buffer limit must be non-zero");
    let state = std::rc::Rc::new(std::cell::RefCell::new(State::new(limit)));
    (
        HyperTransport {
            state: std::rc::Rc::clone(&state),
        },
        RealmTransport { state },
    )
}

struct State {
    reads: VecDeque<Vec<u8>>,
    read_offset: usize,
    read_end: Option<io::Result<()>>,
    read_waker: Option<Waker>,
    writes: VecDeque<Vec<u8>>,
    buffered_write_bytes: usize,
    write_limit: usize,
    write_end: Option<io::Result<()>>,
    write_waker: Option<Waker>,
}

impl State {
    fn new(write_limit: usize) -> Self {
        Self {
            reads: VecDeque::new(),
            read_offset: 0,
            read_end: None,
            read_waker: None,
            writes: VecDeque::new(),
            buffered_write_bytes: 0,
            write_limit,
            write_end: None,
            write_waker: None,
        }
    }
}

/// Hyper-facing async byte stream. It never owns a socket or host authority.
pub(crate) struct HyperTransport {
    state: std::rc::Rc<std::cell::RefCell<State>>,
}

impl AsyncRead for HyperTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        destination: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self.state.borrow_mut();
        if state.reads.front().is_some() {
            let offset = state.read_offset;
            let chunk_length = state.reads.front().expect("front was present").len();
            let length = destination.remaining().min(chunk_length - offset);
            destination.put_slice(
                &state.reads.front().expect("front was present")[offset..offset + length],
            );
            state.read_offset += length;
            if state.read_offset == chunk_length {
                state.reads.pop_front();
                state.read_offset = 0;
            }
            return Poll::Ready(Ok(()));
        }
        if let Some(result) = state.read_end.take() {
            return Poll::Ready(result);
        }
        state.read_waker = Some(context.waker().clone());
        Poll::Pending
    }
}

impl AsyncWrite for HyperTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        source: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.state.borrow_mut();
        if let Some(result) = state.write_end.as_ref() {
            return Poll::Ready(match result {
                Ok(()) => Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "HTTP transport closed",
                )),
                Err(error) => Err(clone_error(error)),
            });
        }
        let capacity = state.write_limit.saturating_sub(state.buffered_write_bytes);
        if capacity == 0 {
            state.write_waker = Some(context.waker().clone());
            return Poll::Pending;
        }
        let length = capacity.min(source.len());
        state.writes.push_back(source[..length].to_vec());
        state.buffered_write_bytes += length;
        Poll::Ready(Ok(length))
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.state.borrow_mut();
        if state.writes.is_empty() {
            return Poll::Ready(match state.write_end.as_ref() {
                Some(Err(error)) => Err(clone_error(error)),
                _ => Ok(()),
            });
        }
        state.write_waker = Some(context.waker().clone());
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(context)
    }
}

/// Realm-facing control half, driven only from sequenced HostOps completions.
pub(crate) struct RealmTransport {
    state: std::rc::Rc<std::cell::RefCell<State>>,
}

impl RealmTransport {
    /// Deliver bytes from one completed `TcpRead` and wake Hyper's connection task.
    pub(crate) fn feed_read(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            self.finish_read();
            return;
        }
        let wake = {
            let mut state = self.state.borrow_mut();
            if state.read_end.is_some() {
                return;
            }
            state.reads.push_back(bytes);
            state.read_waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }

    pub(crate) fn finish_read(&mut self) {
        self.end_read(Ok(()));
    }

    pub(crate) fn fail_read(&mut self, error: io::Error) {
        self.end_read(Err(error));
    }

    fn end_read(&mut self, result: io::Result<()>) {
        let wake = {
            let mut state = self.state.borrow_mut();
            if state.read_end.is_some() {
                return;
            }
            state.read_end = Some(result);
            state.read_waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }

    /// Peek at the next bytes Hyper wants written. They remain buffered until the matching
    /// `TcpWrite` completion is acknowledged, preserving host completion ordering.
    pub(crate) fn next_write(&self) -> Option<Vec<u8>> {
        self.state.borrow().writes.front().cloned()
    }

    /// Retire the front write after its correlated HostOps completion.
    pub(crate) fn acknowledge_write(&mut self) -> bool {
        let wake = {
            let mut state = self.state.borrow_mut();
            let Some(bytes) = state.writes.pop_front() else {
                return false;
            };
            state.buffered_write_bytes -= bytes.len();
            state.write_waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
        true
    }

    pub(crate) fn fail_write(&mut self, error: io::Error) {
        let wake = {
            let mut state = self.state.borrow_mut();
            if state.write_end.is_some() {
                return;
            }
            state.write_end = Some(Err(error));
            state.write_waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

/// Correlates the private Hyper transport with the existing provider-neutral stream driver.
///
/// This is the only conversion point between Hyper's continuously-polled byte stream and
/// HostOps' discrete operations. The caller applies returned actions through either `TokioHost`
/// or `ScriptedHost`; both therefore exercise the same Hyper connection future.
pub(crate) struct HttpTransportDriver {
    stream: StreamDriver,
    transport: RealmTransport,
    write_request: Option<RequestId>,
    write_selected: bool,
    close_requested: bool,
    close_started: bool,
    wrote_bytes: bool,
}

impl HttpTransportDriver {
    pub(crate) fn new(resource: HttpResourceId) -> (HyperTransport, Self) {
        let (hyper, transport) = channel();
        (
            hyper,
            Self {
                stream: StreamDriver::new(resource.resource(), DEFAULT_HIGH_WATER_MARK),
                transport,
                write_request: None,
                write_selected: false,
                close_requested: false,
                close_started: false,
                wrote_bytes: false,
            },
        )
    }

    pub(crate) fn start(&mut self) -> Vec<HttpStreamAction> {
        http_actions(self.stream.start_reading())
    }

    pub(crate) fn started(&mut self, operation: &HttpHostOp, request: RequestId) -> bool {
        if let HttpHostOp::TcpWrite { stream, .. } = operation {
            if stream.resource() != self.stream.resource() {
                return false;
            }
            if !self.write_selected || self.write_request.is_some() {
                return false;
            }
            self.write_selected = false;
            self.write_request = Some(request);
            true
        } else {
            self.stream
                .started(&operation.clone().into_host_op(), request)
        }
    }

    /// Associate the one raw close request authorized by the HTTP runtime.
    pub(crate) fn close_started(&mut self, request: RequestId) -> bool {
        let operation = AsyncHostOp::CloseResource {
            resource: self.stream.resource(),
        };
        self.stream.started(&operation, request)
    }

    /// Pump bytes produced by the most recent Hyper poll into the stream driver's write queue.
    ///
    /// Call this at the realm checkpoint after Tokio has polled local connection tasks. A write
    /// remains at the transport front until its HostOps completion arrives.
    pub(crate) fn pump_write(&mut self) -> Vec<HttpStreamAction> {
        if self.write_selected || self.write_request.is_some() {
            return Vec::new();
        }
        let Some(bytes) = self.transport.next_write() else {
            return if self.close_requested && self.wrote_bytes && !self.close_started {
                self.close_started = true;
                http_actions(self.stream.close_actions())
            } else {
                Vec::new()
            };
        };
        self.write_selected = true;
        self.wrote_bytes = true;
        vec![HttpStreamAction::Start(HttpHostOp::TcpWrite {
            stream: HttpResourceId::new(self.stream.resource()),
            data: bytes,
        })]
    }

    pub(crate) fn completed(&mut self, completion: Completion) -> Option<Vec<HttpStreamAction>> {
        if self.write_request == Some(completion.request) {
            self.write_request = None;
            return Some(match completion.result {
                Ok(kiln_host_abi::AsyncHostValue::Undefined) => {
                    assert!(
                        self.transport.acknowledge_write(),
                        "HTTP write completion must acknowledge one queued protocol frame"
                    );
                    // Acknowledgement wakes Hyper's AsyncWrite waiter. Do not inspect the
                    // transport queue again until the next runtime checkpoint has polled that
                    // waiter; the front frame remains visible until Hyper consumes the wake.
                    Vec::new()
                }
                Ok(_) => {
                    let error = AsyncHostError::new(
                        "ERR_KILN_HOST_VALUE",
                        "write",
                        None,
                        "unexpected host value for write",
                    );
                    self.fail_transport(&error);
                    vec![HttpStreamAction::Error(error)]
                }
                Err(error) => {
                    self.fail_transport(&error);
                    vec![HttpStreamAction::Error(error)]
                }
            });
        }
        let actions = self.stream.completed(completion)?;
        let mut outward = Vec::new();
        for action in actions {
            match action {
                StreamAction::Data(bytes) => self.transport.feed_read(bytes),
                StreamAction::Event(StreamEvent::End) => self.transport.finish_read(),
                StreamAction::Error(error) => {
                    // Once Hyper has completed the connection, a peer may close its socket while
                    // a final transport write completion is still queued. That is normal HTTP/1
                    // shutdown, not a language-visible stream failure.
                    if !self.close_requested
                        && !self.close_started
                        && error.code != "EPIPE"
                        && error.code != "EBADF"
                        && !(error.syscall == "write" && error.message.contains("Broken pipe"))
                    {
                        self.fail_transport(&error);
                        outward.push(HttpStreamAction::Error(error));
                    }
                }
                StreamAction::Event(StreamEvent::Close) => {
                    self.transport.finish_read();
                    outward.push(HttpStreamAction::Event(StreamEvent::Close));
                }
                action => outward.push(http_action(action)),
            }
        }
        // This catches output already produced before the completion. Output produced because
        // this completion woke Hyper is picked up by `pump_write` at the next realm checkpoint.
        outward.extend(self.pump_write());
        Some(outward)
    }

    /// Close the portable resource after Hyper's final buffered write is acknowledged.
    pub(crate) fn close(&mut self) -> Vec<HttpStreamAction> {
        self.close_requested = true;
        if self.write_selected
            || self.write_request.is_some()
            || self.transport.next_write().is_some()
            || self.close_started
        {
            Vec::new()
        } else {
            self.close_started = true;
            http_actions(self.stream.close_actions())
        }
    }

    /// Abort the protocol immediately, canceling both transport directions before closing the
    /// portable resource. Unlike [`Self::close`], this intentionally does not wait for a queued
    /// response write; it is the path for Node's forced destroy and protocol failure/EOF.
    pub(crate) fn abort(&mut self) -> Vec<HttpStreamAction> {
        self.close_requested = true;
        self.close_started = true;
        self.write_selected = false;
        let write_request = self.write_request.take();
        let mut actions = http_actions(self.stream.close_actions());
        if let Some(request) = write_request {
            actions.insert(0, HttpStreamAction::Cancel(request));
        }
        actions
    }

    pub(crate) fn close_requested(&mut self) {
        self.close_requested = true;
    }

    fn fail_transport(&mut self, error: &AsyncHostError) {
        let message = error.to_string();
        self.transport.fail_read(io::Error::other(message.clone()));
        self.transport.fail_write(io::Error::other(message));
        self.write_request = None;
        self.write_selected = false;
    }
}

fn clone_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn realm_completions_drive_reads_eof_and_bounded_write_backpressure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (mut hyper, mut realm) = channel_with_limit(4);
            let task = tokio::task::spawn_local(async move {
                let mut input = Vec::new();
                hyper.read_to_end(&mut input).await.expect("read succeeds");
                assert_eq!(input, b"request");
                hyper.write_all(b"response").await.expect("write succeeds");
                hyper.flush().await.expect("flush succeeds");
            });

            realm.feed_read(b"request".to_vec());
            realm.finish_read();
            tokio::task::yield_now().await;
            assert_eq!(realm.next_write().as_deref(), Some(&b"resp"[..]));
            assert!(realm.acknowledge_write());
            tokio::task::yield_now().await;
            assert_eq!(realm.next_write().as_deref(), Some(&b"onse"[..]));
            assert!(realm.acknowledge_write());
            task.await.expect("transport task joins");
        });
    }

    #[test]
    fn acknowledged_flush_does_not_requeue_the_same_frame() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (mut hyper, mut realm) = channel_with_limit(64);
            let task = tokio::task::spawn_local(async move {
                hyper.write_all(b"one frame").await.expect("write succeeds");
                hyper.flush().await.expect("flush succeeds");
            });
            tokio::task::yield_now().await;
            assert_eq!(realm.next_write().as_deref(), Some(&b"one frame"[..]));
            assert!(realm.acknowledge_write());
            tokio::task::yield_now().await;
            assert!(realm.next_write().is_none());
            task.await.expect("writer joins");
            tokio::task::yield_now().await;
            assert!(
                realm.next_write().is_none(),
                "flush must not replay accepted bytes"
            );
        });
    }

    #[test]
    fn realm_transport_propagates_read_and_write_failures() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (mut hyper, mut realm) = channel();
            realm.fail_read(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "read failed",
            ));
            let mut byte = [0];
            let read = hyper
                .read(&mut byte)
                .await
                .expect_err("read failure surfaces");
            assert_eq!(read.kind(), io::ErrorKind::ConnectionReset);

            realm.fail_write(io::Error::new(io::ErrorKind::BrokenPipe, "write failed"));
            let write = hyper
                .write_all(b"response")
                .await
                .expect_err("write failure surfaces");
            assert_eq!(write.kind(), io::ErrorKind::BrokenPipe);
        });
    }

    #[test]
    fn stream_driver_is_the_single_hostops_path_for_hyper_bytes() {
        use kiln_host_abi::{AsyncHostValue, ResourceId};

        fn completion(request: u64, value: AsyncHostValue) -> Completion {
            Completion {
                request: RequestId::new(request),
                sequence: request,
                result: Ok(value),
            }
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let resource = ResourceId::new(7);
            let http_resource = HttpResourceId::new(resource);
            let (mut hyper, mut driver) = HttpTransportDriver::new(http_resource);
            let start = driver.start();
            let [HttpStreamAction::Start(read)] = start.as_slice() else {
                panic!("driver starts exactly one read")
            };
            assert!(driver.started(read, RequestId::new(1)));
            assert!(
                driver
                    .completed(completion(1, AsyncHostValue::Bytes(b"request".to_vec())))
                    .expect("completion belongs to driver")
                    .iter()
                    .any(|action| matches!(
                        action,
                        HttpStreamAction::Start(HttpHostOp::TcpRead { .. })
                    ))
            );

            let mut input = [0; 7];
            hyper
                .read_exact(&mut input)
                .await
                .expect("read crosses bridge");
            assert_eq!(&input, b"request");
            hyper.write_all(b"response").await.expect("write buffers");
            let pumped = driver.pump_write();
            let [HttpStreamAction::Start(write)] = pumped.as_slice() else {
                panic!("driver starts exactly one write")
            };
            assert!(matches!(write, HttpHostOp::TcpWrite { data, .. } if data == b"response"));
            let wrong_write = HttpHostOp::TcpWrite {
                stream: HttpResourceId::new(ResourceId::new(70)),
                data: b"response".to_vec(),
            };
            assert!(
                !driver.started(&wrong_write, RequestId::new(2)),
                "an HTTP write cannot be registered on another connection"
            );
            assert!(driver.started(write, RequestId::new(2)));
            let completed = driver
                .completed(completion(2, AsyncHostValue::Undefined))
                .expect("completion belongs to driver");
            assert!(
                completed.is_empty(),
                "one protocol frame writes once: {completed:?}"
            );
            assert!(driver.pump_write().is_empty());
        });
    }

    #[test]
    fn abort_cancels_pending_http_write_before_closing_resource() {
        use kiln_host_abi::ResourceId;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let resource = ResourceId::new(8);
            let http_resource = HttpResourceId::new(resource);
            let (mut hyper, mut driver) = HttpTransportDriver::new(http_resource);
            hyper.write_all(b"response").await.expect("write buffers");
            let pumped = driver.pump_write();
            let [HttpStreamAction::Start(write)] = pumped.as_slice() else {
                panic!("driver starts exactly one HTTP write");
            };
            assert!(driver.started(write, RequestId::new(1)));

            assert_eq!(
                driver.abort(),
                vec![
                    HttpStreamAction::Cancel(RequestId::new(1)),
                    HttpStreamAction::Close(HttpCloseCapability::new(http_resource)),
                ]
            );
        });
    }
}

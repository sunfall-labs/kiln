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
    write_in_flight: bool,
    close_requested: bool,
    close_started: bool,
    wrote_bytes: bool,
}

impl HttpTransportDriver {
    pub(crate) fn new(resource: ResourceId) -> (HyperTransport, Self) {
        let (hyper, transport) = channel();
        (
            hyper,
            Self {
                stream: StreamDriver::new(resource, DEFAULT_HIGH_WATER_MARK),
                transport,
                write_in_flight: false,
                close_requested: false,
                close_started: false,
                wrote_bytes: false,
            },
        )
    }

    pub(crate) fn start(&mut self) -> Vec<StreamAction> {
        self.stream.start_reading()
    }

    pub(crate) fn started(&mut self, operation: &AsyncHostOp, request: RequestId) -> bool {
        self.stream.started(operation, request)
    }

    /// Pump bytes produced by the most recent Hyper poll into the stream driver's write queue.
    ///
    /// Call this at the realm checkpoint after Tokio has polled local connection tasks. A write
    /// remains at the transport front until its HostOps completion arrives.
    pub(crate) fn pump_write(&mut self) -> Vec<StreamAction> {
        if self.write_in_flight {
            return Vec::new();
        }
        let Some(bytes) = self.transport.next_write() else {
            return if self.close_requested && self.wrote_bytes && !self.close_started {
                self.close_started = true;
                self.stream.close_actions()
            } else {
                Vec::new()
            };
        };
        let (_, actions) = self.stream.write(bytes);
        self.write_in_flight = !actions.is_empty();
        self.wrote_bytes |= self.write_in_flight;
        actions
    }

    pub(crate) fn completed(&mut self, completion: Completion) -> Option<Vec<StreamAction>> {
        let write_completed = self.stream.is_write_completion(completion.request);
        let write_succeeded = write_completed && completion.result.is_ok();
        if write_succeeded {
            self.write_in_flight = false;
            assert!(
                self.transport.acknowledge_write(),
                "HTTP write completion had no queued transport bytes"
            );
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
                        outward.push(StreamAction::Error(error));
                    }
                }
                StreamAction::Event(StreamEvent::Close) => {
                    self.transport.finish_read();
                    outward.push(StreamAction::Event(StreamEvent::Close));
                }
                action => outward.push(action),
            }
        }
        if write_completed && self.close_requested && !self.close_started {
            self.close_started = true;
            outward.extend(self.stream.close_actions());
            return Some(outward);
        }
        // This catches output already produced before the completion. Output produced because
        // this completion woke Hyper is picked up by `pump_write` at the next realm checkpoint.
        outward.extend(self.pump_write());
        Some(outward)
    }

    /// Close the portable resource after Hyper's final buffered write is acknowledged.
    pub(crate) fn close(&mut self) -> Vec<StreamAction> {
        self.close_requested = true;
        if self.write_in_flight || self.transport.next_write().is_some() || self.close_started {
            Vec::new()
        } else {
            self.close_started = true;
            self.stream.close_actions()
        }
    }

    pub(crate) fn close_requested(&mut self) {
        self.close_requested = true;
    }

    fn fail_transport(&mut self, error: &AsyncHostError) {
        let message = error.to_string();
        self.transport.fail_read(io::Error::other(message.clone()));
        self.transport.fail_write(io::Error::other(message));
        self.write_in_flight = false;
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
            let (mut hyper, mut driver) = HttpTransportDriver::new(resource);
            let start = driver.start();
            let [StreamAction::Start(read)] = start.as_slice() else {
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
                        StreamAction::Start(AsyncHostOp::TcpRead { .. })
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
            let [StreamAction::Start(write)] = pumped.as_slice() else {
                panic!("driver starts exactly one write")
            };
            assert!(matches!(write, AsyncHostOp::TcpWrite { data, .. } if data == b"response"));
            assert!(driver.started(write, RequestId::new(2)));
            assert!(
                driver
                    .completed(completion(2, AsyncHostValue::Undefined))
                    .expect("completion belongs to driver")
                    .is_empty()
            );
        });
    }
}

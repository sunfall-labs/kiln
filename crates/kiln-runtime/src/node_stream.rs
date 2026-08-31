//! Executor-independent Node stream state.
//!
//! Tokio owns readiness and bytes. This module owns only the ordering and accounting Node makes
//! observable: buffered-write pressure, `drain`, EOF, half-close, and final `close`. Keeping the
//! state machine free of Tokio and JavaScript values lets interpreter and native adapters share
//! it and lets deterministic scripted hosts exercise the same transitions as live sockets.

use std::collections::VecDeque;

use kiln_host_abi::{
    AsyncHostError, AsyncHostOp, AsyncHostValue, Completion, RequestId, ResourceId,
};

/// Default writable high-water mark used by Node byte streams.
pub const DEFAULT_HIGH_WATER_MARK: usize = 16 * 1024;

/// One event an adapter must publish after a state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent {
    Drain,
    End,
    Finish,
    Error,
    Close,
}

/// One side effect selected by [`StreamDriver`].
///
/// The caller performs host operations and publishes language events. That inversion is what
/// lets a live Tokio host and a deterministic scripted host exercise exactly the same driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamAction {
    Start(AsyncHostOp),
    Cancel(RequestId),
    Data(Vec<u8>),
    Event(StreamEvent),
    Error(AsyncHostError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Read,
    Write,
    Shutdown,
    Close,
    Ref,
}

/// Correlates host requests for one duplex resource and drives its Node-visible state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDriver {
    resource: ResourceId,
    state: DuplexState<Vec<u8>>,
    requests: Vec<(RequestId, PendingKind)>,
    reading: bool,
    closing: bool,
    read_size: usize,
}

impl StreamDriver {
    pub fn new(resource: ResourceId, high_water_mark: usize) -> Self {
        Self {
            resource,
            state: DuplexState::new(high_water_mark),
            requests: Vec::new(),
            reading: false,
            closing: false,
            read_size: high_water_mark.max(1),
        }
    }

    pub const fn state(&self) -> &DuplexState<Vec<u8>> {
        &self.state
    }

    pub const fn resource(&self) -> ResourceId {
        self.resource
    }

    pub fn start_reading(&mut self) -> Vec<StreamAction> {
        self.reading = true;
        self.next_read_action().into_iter().collect()
    }

    pub fn pause_reading(&mut self) {
        self.reading = false;
    }

    /// Resume delivery after a language-level readable paused it.
    ///
    /// A read already in flight is deliberately left alone: its completion is retained by the
    /// adapter, while this method only selects a new host read when none exists. That keeps
    /// cancellation and transport readiness out of Node's user-visible pause semantics.
    pub fn resume_reading(&mut self) -> Vec<StreamAction> {
        self.reading = true;
        self.next_read_action().into_iter().collect()
    }

    pub const fn is_reading(&self) -> bool {
        self.reading
    }

    /// Whether a transport read is already registered for this stream.
    ///
    /// Adapters use this while applying a completion batch: a `data` listener may synchronously
    /// call `resume()` while that same batch already contains the next read action. Registering
    /// both would make one language transition own two concurrent socket reads.
    pub fn has_pending_read(&self) -> bool {
        self.requests
            .iter()
            .any(|(_, kind)| matches!(kind, PendingKind::Read))
    }

    pub fn write(&mut self, data: Vec<u8>) -> (WriteDisposition, Vec<StreamAction>) {
        let disposition = self.state.enqueue_write(data.clone(), data.len());
        let actions = if disposition.submit_now {
            vec![StreamAction::Start(AsyncHostOp::TcpWrite {
                stream: self.resource,
                data,
            })]
        } else {
            Vec::new()
        };
        (disposition, actions)
    }

    pub fn end(&mut self, data: Option<Vec<u8>>) -> (WriteDisposition, Vec<StreamAction>) {
        let (disposition, mut actions) = match data {
            Some(data) if !data.is_empty() => self.write(data),
            _ => (
                WriteDisposition {
                    ready: true,
                    submit_now: false,
                },
                Vec::new(),
            ),
        };
        if self.state.request_end() {
            actions.push(StreamAction::Start(AsyncHostOp::TcpShutdown {
                stream: self.resource,
            }));
        }
        (disposition, actions)
    }

    pub fn set_referenced(&self, referenced: bool) -> StreamAction {
        StreamAction::Start(AsyncHostOp::SetResourceReferenced {
            resource: self.resource,
            referenced,
        })
    }

    pub fn close(&self) -> StreamAction {
        StreamAction::Start(AsyncHostOp::CloseResource {
            resource: self.resource,
        })
    }

    /// Cancel all in-flight operations before closing this resource.
    ///
    /// The close action can be selected synchronously from a language callback while a read is
    /// still waiting on the peer. Retiring those request identities is part of close: otherwise
    /// the realm's host drive keeps waiting for a completion that can no longer be observed.
    pub fn close_actions(&mut self) -> Vec<StreamAction> {
        if self.closing {
            return Vec::new();
        }
        self.closing = true;
        let mut actions: Vec<_> = self
            .requests
            .iter()
            .map(|(request, _)| StreamAction::Cancel(*request))
            .collect();
        self.requests.clear();
        actions.push(self.close());
        actions
    }

    pub const fn is_closing(&self) -> bool {
        self.closing
    }

    /// Associate the request identity returned for a selected host operation.
    pub fn started(&mut self, operation: &AsyncHostOp, request: RequestId) -> bool {
        let kind = match operation {
            AsyncHostOp::TcpRead { stream, .. } if *stream == self.resource => PendingKind::Read,
            AsyncHostOp::TcpWrite { stream, .. } if *stream == self.resource => {
                if !self.state.write_started(request) {
                    return false;
                }
                PendingKind::Write
            }
            AsyncHostOp::TcpShutdown { stream } if *stream == self.resource => {
                PendingKind::Shutdown
            }
            AsyncHostOp::CloseResource { resource } if *resource == self.resource => {
                PendingKind::Close
            }
            AsyncHostOp::SetResourceReferenced { resource, .. } if *resource == self.resource => {
                PendingKind::Ref
            }
            _ => return false,
        };
        if self
            .requests
            .iter()
            .any(|(existing, _)| *existing == request)
        {
            return false;
        }
        self.requests.push((request, kind));
        true
    }

    /// Consume one completion if it belongs to this stream.
    pub fn completed(&mut self, completion: Completion) -> Option<Vec<StreamAction>> {
        let index = self
            .requests
            .iter()
            .position(|(request, _)| *request == completion.request)?;
        let (_, kind) = self.requests.remove(index);
        let value = match completion.result {
            Ok(value) => value,
            Err(error) => {
                let mut actions = vec![StreamAction::Error(error)];
                actions.extend(
                    self.state
                        .close_resource()
                        .into_iter()
                        .map(StreamAction::Event),
                );
                return Some(actions);
            }
        };
        let mut actions = Vec::new();
        match kind {
            PendingKind::Read => match value {
                AsyncHostValue::Bytes(bytes) if bytes.is_empty() => {
                    actions.extend(self.state.read_eof().into_iter().map(StreamAction::Event));
                }
                AsyncHostValue::Bytes(bytes) => {
                    actions.push(StreamAction::Data(bytes));
                    if let Some(read) = self.next_read_action() {
                        actions.push(read);
                    }
                }
                _ => actions.push(StreamAction::Error(unexpected_value("read"))),
            },
            PendingKind::Write => {
                if !matches!(value, AsyncHostValue::Undefined) {
                    actions.push(StreamAction::Error(unexpected_value("write")));
                    return Some(actions);
                }
                let (events, submit_next) = self.state.write_completed(completion.request);
                actions.extend(events.into_iter().map(StreamAction::Event));
                if submit_next {
                    let data = self
                        .state
                        .next_write()
                        .expect("state selected a queued write")
                        .clone();
                    actions.push(StreamAction::Start(AsyncHostOp::TcpWrite {
                        stream: self.resource,
                        data,
                    }));
                } else if self.state.end_requested() && !self.state.write_closed() {
                    actions.push(StreamAction::Start(AsyncHostOp::TcpShutdown {
                        stream: self.resource,
                    }));
                }
            }
            PendingKind::Shutdown => {
                if matches!(value, AsyncHostValue::Undefined) {
                    actions.extend(
                        self.state
                            .shutdown_completed()
                            .into_iter()
                            .map(StreamAction::Event),
                    );
                } else {
                    actions.push(StreamAction::Error(unexpected_value("shutdown")));
                }
            }
            PendingKind::Close => {
                if matches!(value, AsyncHostValue::Undefined) {
                    actions.extend(
                        self.state
                            .close_resource()
                            .into_iter()
                            .map(StreamAction::Event),
                    );
                } else {
                    actions.push(StreamAction::Error(unexpected_value("close")));
                }
            }
            PendingKind::Ref => {
                if !matches!(value, AsyncHostValue::Undefined) {
                    actions.push(StreamAction::Error(unexpected_value("ref")));
                }
            }
        }
        Some(actions)
    }

    /// Whether a completion belongs to the currently submitted transport write.
    ///
    /// Protocol adapters use this before consuming the completion so their own bounded byte
    /// queues can retire exactly the write acknowledged by HostOps. This exposes correlation,
    /// not Tokio or protocol state.
    pub(crate) fn is_write_completion(&self, request: RequestId) -> bool {
        self.requests
            .iter()
            .any(|(known, kind)| *known == request && matches!(kind, PendingKind::Write))
    }

    fn next_read_action(&self) -> Option<StreamAction> {
        (self.reading
            && !self.closing
            && !self.state.read_closed
            && !self.state.resource_closed
            && !self
                .requests
                .iter()
                .any(|(_, kind)| matches!(kind, PendingKind::Read)))
        .then_some(StreamAction::Start(AsyncHostOp::TcpRead {
            stream: self.resource,
            max_bytes: self.read_size,
        }))
    }
}

fn unexpected_value(syscall: &str) -> AsyncHostError {
    AsyncHostError::new(
        "ERR_KILN_HOST_VALUE",
        syscall,
        None,
        "host returned an incompatible value for the stream operation",
    )
}

/// Result of accepting one user write into the stream queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteDisposition {
    /// Whether `write()` returns true. False means the writer must wait for `drain`.
    pub ready: bool,
    /// Whether this write can be submitted to the host immediately.
    pub submit_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingWrite<T> {
    bytes: usize,
    payload: T,
    request: Option<RequestId>,
}

/// Node-visible state for one duplex byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplexState<T = Vec<u8>> {
    high_water_mark: usize,
    buffered_bytes: usize,
    writes: VecDeque<PendingWrite<T>>,
    waiting_for_drain: bool,
    read_closed: bool,
    end_requested: bool,
    write_closed: bool,
    resource_closed: bool,
}

impl<T> Default for DuplexState<T> {
    fn default() -> Self {
        Self::new(DEFAULT_HIGH_WATER_MARK)
    }
}

impl<T> DuplexState<T> {
    pub fn new(high_water_mark: usize) -> Self {
        Self {
            high_water_mark,
            buffered_bytes: 0,
            writes: VecDeque::new(),
            waiting_for_drain: false,
            read_closed: false,
            end_requested: false,
            write_closed: false,
            resource_closed: false,
        }
    }

    pub const fn buffered_bytes(&self) -> usize {
        self.buffered_bytes
    }

    pub const fn read_closed(&self) -> bool {
        self.read_closed
    }

    pub const fn write_closed(&self) -> bool {
        self.write_closed
    }

    pub const fn end_requested(&self) -> bool {
        self.end_requested
    }

    pub const fn resource_closed(&self) -> bool {
        self.resource_closed
    }

    /// Queue one write. The adapter submits only the queue head and calls [`Self::write_started`]
    /// with its request identity; subsequent writes remain ordered without serialising Tokio's
    /// unrelated work.
    pub fn enqueue_write(&mut self, payload: T, bytes: usize) -> WriteDisposition {
        if self.end_requested || self.write_closed || self.resource_closed {
            return WriteDisposition {
                ready: false,
                submit_now: false,
            };
        }
        let submit_now = self.writes.is_empty();
        self.buffered_bytes = self.buffered_bytes.saturating_add(bytes);
        self.writes.push_back(PendingWrite {
            bytes,
            payload,
            request: None,
        });
        let ready = self.buffered_bytes < self.high_water_mark;
        self.waiting_for_drain |= !ready;
        WriteDisposition { ready, submit_now }
    }

    pub fn write_started(&mut self, request: RequestId) -> bool {
        let Some(head) = self.writes.front_mut() else {
            return false;
        };
        if head.request.is_some() {
            return false;
        }
        head.request = Some(request);
        true
    }

    pub fn next_write(&self) -> Option<&T> {
        self.writes
            .front()
            .filter(|write| write.request.is_none())
            .map(|write| &write.payload)
    }

    /// Complete the ordered queue head and return events plus whether another write is ready to
    /// submit. A mismatched completion is ignored: it belongs to another stream or stale request.
    pub fn write_completed(&mut self, request: RequestId) -> (Vec<StreamEvent>, bool) {
        let Some(head) = self.writes.front() else {
            return (Vec::new(), false);
        };
        if head.request != Some(request) {
            return (Vec::new(), false);
        }
        let completed = self.writes.pop_front().expect("front was present");
        self.buffered_bytes = self.buffered_bytes.saturating_sub(completed.bytes);
        let mut events = Vec::new();
        if self.waiting_for_drain && self.buffered_bytes < self.high_water_mark {
            self.waiting_for_drain = false;
            events.push(StreamEvent::Drain);
        }
        let submit_next = self
            .writes
            .front()
            .is_some_and(|write| write.request.is_none());
        (events, submit_next)
    }

    /// Record `end()`. Shutdown can be submitted only after the write queue drains.
    pub fn request_end(&mut self) -> bool {
        if self.end_requested || self.write_closed || self.resource_closed {
            return false;
        }
        self.end_requested = true;
        self.writes.is_empty()
    }

    pub fn shutdown_completed(&mut self) -> Vec<StreamEvent> {
        if self.write_closed {
            return Vec::new();
        }
        self.write_closed = true;
        let mut events = vec![StreamEvent::Finish];
        self.maybe_close(&mut events);
        events
    }

    pub fn read_eof(&mut self) -> Vec<StreamEvent> {
        if self.read_closed {
            return Vec::new();
        }
        self.read_closed = true;
        let mut events = vec![StreamEvent::End];
        self.maybe_close(&mut events);
        events
    }

    /// Explicit destruction closes immediately; natural close waits for both halves.
    pub fn close_resource(&mut self) -> Vec<StreamEvent> {
        if self.resource_closed {
            return Vec::new();
        }
        self.resource_closed = true;
        vec![StreamEvent::Close]
    }

    fn maybe_close(&mut self, events: &mut Vec<StreamEvent>) {
        if self.read_closed && self.write_closed && !self.resource_closed {
            self.resource_closed = true;
            events.push(StreamEvent::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(request: u64, value: AsyncHostValue) -> Completion {
        Completion {
            request: RequestId::new(request),
            sequence: request,
            result: Ok(value),
        }
    }

    fn start(driver: &mut StreamDriver, action: &StreamAction, request: u64) {
        let StreamAction::Start(operation) = action else {
            panic!("expected host operation, got {action:?}");
        };
        assert!(driver.started(operation, RequestId::new(request)));
    }

    #[test]
    fn driver_serialises_writes_and_shuts_down_only_after_the_queue_drains() {
        let resource = ResourceId::new(9);
        let mut driver = StreamDriver::new(resource, 4);
        let (first, first_actions) = driver.write(b"abc".to_vec());
        assert!(first.ready);
        assert_eq!(
            first_actions,
            vec![StreamAction::Start(AsyncHostOp::TcpWrite {
                stream: resource,
                data: b"abc".to_vec(),
            })]
        );
        start(&mut driver, &first_actions[0], 1);

        let (second, second_actions) = driver.end(Some(b"de".to_vec()));
        assert!(!second.ready);
        assert!(second_actions.is_empty());
        assert!(driver.state().end_requested());
        assert_eq!(driver.state().buffered_bytes(), 5);

        let actions = driver
            .completed(completion(1, AsyncHostValue::Undefined))
            .expect("write belongs to stream");
        assert_eq!(
            actions,
            vec![
                StreamAction::Event(StreamEvent::Drain),
                StreamAction::Start(AsyncHostOp::TcpWrite {
                    stream: resource,
                    data: b"de".to_vec(),
                }),
            ]
        );
        start(&mut driver, &actions[1], 2);

        let actions = driver
            .completed(completion(2, AsyncHostValue::Undefined))
            .expect("second write belongs to stream");
        assert_eq!(
            actions,
            vec![StreamAction::Start(AsyncHostOp::TcpShutdown {
                stream: resource,
            })]
        );
        start(&mut driver, &actions[0], 3);
        assert_eq!(
            driver.completed(completion(3, AsyncHostValue::Undefined)),
            Some(vec![StreamAction::Event(StreamEvent::Finish)])
        );
    }

    #[test]
    fn driver_keeps_one_read_in_flight_and_turns_eof_into_end() {
        let resource = ResourceId::new(4);
        let mut driver = StreamDriver::new(resource, 8);
        let actions = driver.start_reading();
        assert_eq!(
            actions,
            vec![StreamAction::Start(AsyncHostOp::TcpRead {
                stream: resource,
                max_bytes: 8,
            })]
        );
        start(&mut driver, &actions[0], 10);
        assert!(
            driver.start_reading().is_empty(),
            "one read stays in flight"
        );

        let actions = driver
            .completed(completion(10, AsyncHostValue::Bytes(b"hello".to_vec())))
            .expect("read belongs to stream");
        assert_eq!(
            actions,
            vec![
                StreamAction::Data(b"hello".to_vec()),
                StreamAction::Start(AsyncHostOp::TcpRead {
                    stream: resource,
                    max_bytes: 8,
                }),
            ]
        );
        start(&mut driver, &actions[1], 11);
        assert_eq!(
            driver.completed(completion(11, AsyncHostValue::Bytes(Vec::new()))),
            Some(vec![StreamAction::Event(StreamEvent::End)])
        );
        assert!(driver.state().read_closed());
    }

    #[test]
    fn close_actions_cancel_in_flight_read_before_closing_resource() {
        let resource = ResourceId::new(6);
        let mut driver = StreamDriver::new(resource, 8);
        let actions = driver.start_reading();
        start(&mut driver, &actions[0], 13);
        assert_eq!(
            driver.close_actions(),
            vec![
                StreamAction::Cancel(RequestId::new(13)),
                StreamAction::Start(AsyncHostOp::CloseResource { resource }),
            ]
        );
        assert!(driver.is_closing());
        assert!(driver.close_actions().is_empty());
        assert!(driver.start_reading().is_empty());
        assert_eq!(
            driver.completed(completion(13, AsyncHostValue::Bytes(b"stale".to_vec()))),
            None
        );
    }

    #[test]
    fn pause_stops_the_next_read_and_resume_restarts_it() {
        let resource = ResourceId::new(5);
        let mut driver = StreamDriver::new(resource, 8);
        let actions = driver.start_reading();
        start(&mut driver, &actions[0], 12);
        driver.pause_reading();

        assert_eq!(
            driver.completed(completion(12, AsyncHostValue::Bytes(b"held".to_vec()))),
            Some(vec![StreamAction::Data(b"held".to_vec())])
        );
        assert!(!driver.is_reading());
        assert_eq!(
            driver.resume_reading(),
            vec![StreamAction::Start(AsyncHostOp::TcpRead {
                stream: resource,
                max_bytes: 8,
            })]
        );
    }

    #[test]
    fn driver_ignores_foreign_completions_and_surfaces_host_errors() {
        let resource = ResourceId::new(2);
        let mut driver = StreamDriver::new(resource, 16);
        let (_, actions) = driver.write(vec![1]);
        start(&mut driver, &actions[0], 20);
        assert_eq!(
            driver.completed(completion(99, AsyncHostValue::Undefined)),
            None
        );
        let error = AsyncHostError::new("EPIPE", "write", None, "broken pipe");
        assert_eq!(
            driver.completed(Completion {
                request: RequestId::new(20),
                sequence: 20,
                result: Err(error.clone()),
            }),
            Some(vec![
                StreamAction::Error(error),
                StreamAction::Event(StreamEvent::Close),
            ])
        );
        assert!(driver.state().resource_closed());
    }

    #[test]
    fn high_water_mark_applies_to_total_queued_bytes_and_drain_fires_once() {
        let mut stream = DuplexState::new(8);
        assert_eq!(
            stream.enqueue_write(vec![0; 4], 4),
            WriteDisposition {
                ready: true,
                submit_now: true
            }
        );
        assert!(stream.write_started(RequestId::new(1)));
        assert_eq!(
            stream.enqueue_write(vec![0; 4], 4),
            WriteDisposition {
                ready: false,
                submit_now: false
            }
        );
        assert_eq!(stream.buffered_bytes(), 8);
        assert_eq!(
            stream.write_completed(RequestId::new(1)),
            (vec![StreamEvent::Drain], true)
        );
        assert!(stream.write_started(RequestId::new(2)));
        assert_eq!(
            stream.write_completed(RequestId::new(2)),
            (Vec::new(), false)
        );
        assert_eq!(stream.buffered_bytes(), 0);
    }

    #[test]
    fn writes_complete_strictly_in_submission_order() {
        let mut stream = DuplexState::default();
        stream.enqueue_write(vec![1], 1);
        stream.write_started(RequestId::new(10));
        stream.enqueue_write(vec![2; 2], 2);
        assert_eq!(
            stream.write_completed(RequestId::new(11)),
            (Vec::new(), false)
        );
        assert_eq!(stream.buffered_bytes(), 3);
        assert_eq!(
            stream.write_completed(RequestId::new(10)),
            (Vec::new(), true)
        );
    }

    #[test]
    fn natural_duplex_close_orders_end_finish_close() {
        let mut stream = DuplexState::<Vec<u8>>::default();
        assert!(stream.request_end());
        assert_eq!(stream.read_eof(), vec![StreamEvent::End]);
        assert_eq!(
            stream.shutdown_completed(),
            vec![StreamEvent::Finish, StreamEvent::Close]
        );
        assert!(stream.read_closed());
        assert!(stream.write_closed());
        assert!(stream.resource_closed());
    }

    #[test]
    fn explicit_close_is_exactly_once() {
        let mut stream = DuplexState::<Vec<u8>>::default();
        assert_eq!(stream.close_resource(), vec![StreamEvent::Close]);
        assert!(stream.close_resource().is_empty());
        assert!(stream.read_eof().contains(&StreamEvent::End));
        assert_eq!(stream.shutdown_completed(), vec![StreamEvent::Finish]);
    }
}

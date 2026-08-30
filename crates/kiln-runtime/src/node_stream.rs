//! Executor-independent Node stream state.
//!
//! Tokio owns readiness and bytes. This module owns only the ordering and accounting Node makes
//! observable: buffered-write pressure, `drain`, EOF, half-close, and final `close`. Keeping the
//! state machine free of Tokio and JavaScript values lets interpreter and native adapters share
//! it and lets deterministic scripted hosts exercise the same transitions as live sockets.

use std::collections::VecDeque;

use kiln_host_abi::RequestId;

/// Default writable high-water mark used by Node byte streams.
pub const DEFAULT_HIGH_WATER_MARK: usize = 16 * 1024;

/// One event an adapter must publish after a state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent {
    Drain,
    End,
    Finish,
    Close,
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
struct PendingWrite {
    bytes: usize,
    request: Option<RequestId>,
}

/// Node-visible state for one duplex byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplexState {
    high_water_mark: usize,
    buffered_bytes: usize,
    writes: VecDeque<PendingWrite>,
    waiting_for_drain: bool,
    read_closed: bool,
    end_requested: bool,
    write_closed: bool,
    resource_closed: bool,
}

impl Default for DuplexState {
    fn default() -> Self {
        Self::new(DEFAULT_HIGH_WATER_MARK)
    }
}

impl DuplexState {
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

    pub const fn resource_closed(&self) -> bool {
        self.resource_closed
    }

    /// Queue one write. The adapter submits only the queue head and calls [`Self::write_started`]
    /// with its request identity; subsequent writes remain ordered without serialising Tokio's
    /// unrelated work.
    pub fn enqueue_write(&mut self, bytes: usize) -> WriteDisposition {
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

    #[test]
    fn high_water_mark_applies_to_total_queued_bytes_and_drain_fires_once() {
        let mut stream = DuplexState::new(8);
        assert_eq!(
            stream.enqueue_write(4),
            WriteDisposition {
                ready: true,
                submit_now: true
            }
        );
        assert!(stream.write_started(RequestId::new(1)));
        assert_eq!(
            stream.enqueue_write(4),
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
        stream.enqueue_write(1);
        stream.write_started(RequestId::new(10));
        stream.enqueue_write(2);
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
        let mut stream = DuplexState::default();
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
        let mut stream = DuplexState::default();
        assert_eq!(stream.close_resource(), vec![StreamEvent::Close]);
        assert!(stream.close_resource().is_empty());
        assert!(stream.read_eof().contains(&StreamEvent::End));
        assert_eq!(stream.shutdown_completed(), vec![StreamEvent::Finish]);
    }
}

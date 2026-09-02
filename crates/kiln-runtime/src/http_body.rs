//! Node-visible response-body accounting below `ServerResponse` and above Hyper framing.

use std::cell::RefCell;
use std::rc::Rc;
use std::task::Waker;

use kiln_host_abi::RequestId;

use crate::node_stream::{DEFAULT_HIGH_WATER_MARK, DuplexState, StreamEvent, WriteDisposition};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BodyEvent {
    Drain,
    Finish,
}

/// One request-scoped writable body. Hyper consumes ordered chunks; this driver owns only the
/// semantics Node exposes synchronously from `write()` and asynchronously through `drain`.
#[derive(Debug, Clone)]
pub(crate) struct ResponseBodyDriver {
    shared: Rc<RefCell<Shared>>,
}

/// Where a body's `drain` and `finish` go: a channel the old checkpoint drains, or a callback
/// the direct engine defers to its next safe point (ADR 0042).
#[derive(Clone)]
pub(crate) enum BodySink {
    Channel(tokio::sync::mpsc::UnboundedSender<BodyEvent>),
    Direct(Rc<dyn Fn(BodyEvent)>),
}

impl std::fmt::Debug for BodySink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Channel(_) => formatter.write_str("BodySink::Channel"),
            Self::Direct(_) => formatter.write_str("BodySink::Direct"),
        }
    }
}

impl BodySink {
    fn send(&self, event: BodyEvent) {
        match self {
            Self::Channel(sender) => {
                let _ = sender.send(event);
            }
            Self::Direct(callback) => callback(event),
        }
    }
}

#[derive(Debug, Clone)]
struct Shared {
    state: DuplexState<Vec<u8>>,
    next_submission: u64,
    in_flight: Option<RequestId>,
    finish_emitted: bool,
    waker: Option<Waker>,
    events: Option<BodySink>,
}

impl Shared {
    /// The realm has ended the body and every byte of it is in Hyper's hands.
    ///
    /// This is deliberately not "Hyper polled past the end". A response that declares a
    /// Content-Length lets Hyper stop polling the moment it holds the bytes it promised, so a
    /// body that waited for one more poll would never finish, Node would never see `finish`, and
    /// `server.close()` would wait on a response that has already been sent.
    fn complete(&self) -> bool {
        self.state.end_requested() && self.state.unsubmitted_writes() == 0
    }

    /// Announce completion exactly once, and only where there is somewhere to announce it.
    fn finish(&mut self) {
        if self.finish_emitted {
            return;
        }
        let Some(events) = &self.events else {
            return;
        };
        self.finish_emitted = true;
        events.send(BodyEvent::Finish);
    }
}

impl Default for ResponseBodyDriver {
    fn default() -> Self {
        Self::new(DEFAULT_HIGH_WATER_MARK)
    }
}

impl ResponseBodyDriver {
    pub(crate) fn new(high_water_mark: usize) -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                state: DuplexState::new(high_water_mark),
                next_submission: 1,
                in_flight: None,
                finish_emitted: false,
                waker: None,
                events: None,
            })),
        }
    }

    pub(crate) fn write(&mut self, data: Vec<u8>) -> WriteDisposition {
        let disposition = self
            .shared
            .borrow_mut()
            .state
            .enqueue_write(data.clone(), data.len());
        self.wake();
        disposition
    }

    pub(crate) fn end(&mut self, data: Vec<u8>) -> WriteDisposition {
        let disposition = if data.is_empty() {
            WriteDisposition {
                ready: self.shared.borrow().state.buffered_bytes() == 0,
                submit_now: false,
            }
        } else {
            self.write(data)
        };
        {
            let mut shared = self.shared.borrow_mut();
            shared.state.request_end();
            if shared.complete() {
                shared.finish();
            }
        }
        self.wake();
        disposition
    }

    /// Whether Hyper holds the whole body. Read from the driver rather than from the event
    /// channel so that asking does not consume the `finish` the realm still has to deliver.
    pub(crate) fn finished(&self) -> bool {
        self.shared.borrow().finish_emitted
    }

    /// A response that cannot carry a body - a HEAD request, or a status with no body - never has
    /// its frames polled, so whatever the realm wrote is dropped on the floor and the body is
    /// complete the moment it is ended. Without this it would wait for a poll that never comes.
    pub(crate) fn finish_without_body(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.state.request_end();
        shared.finish();
    }

    pub(crate) fn body(
        &self,
        events: tokio::sync::mpsc::UnboundedSender<BodyEvent>,
    ) -> ResponseBody {
        self.set_sink(BodySink::Channel(events));
        self.into_body()
    }

    /// Route this body's events to `sink`. A body ended before it was handed over is already
    /// complete and reports its finish at once.
    pub(crate) fn set_sink(&self, sink: BodySink) {
        let mut shared = self.shared.borrow_mut();
        shared.events = Some(sink);
        if shared.complete() {
            shared.finish();
        }
    }

    /// The Hyper body over this driver's state, sharing it with the driver.
    pub(crate) fn into_body(&self) -> ResponseBody {
        ResponseBody {
            shared: Rc::clone(&self.shared),
        }
    }

    fn wake(&self) {
        if let Some(waker) = self.shared.borrow_mut().waker.take() {
            waker.wake();
        }
    }
}

pub(crate) struct ResponseBody {
    shared: Rc<RefCell<Shared>>,
}

impl hyper::body::Body for ResponseBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let mut shared = self.shared.borrow_mut();
        if let Some(request) = shared.in_flight.take() {
            let (events, _) = shared.state.write_completed(request);
            for event in events {
                if event == StreamEvent::Drain
                    && let Some(sink) = &shared.events
                {
                    sink.send(BodyEvent::Drain);
                }
            }
            if shared.complete() {
                shared.finish();
            }
        }
        if shared.in_flight.is_none() {
            let request = RequestId::new(shared.next_submission.max(1));
            shared.next_submission = shared.next_submission.wrapping_add(1).max(1);
            if let Some(payload) = shared.state.submit_next(request).cloned() {
                shared.in_flight = Some(request);
                // Handing over the last frame completes the body, whether or not Hyper polls
                // again. With a declared length it will not.
                if shared.complete() {
                    shared.finish();
                }
                return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(
                    bytes::Bytes::from(payload),
                ))));
            }
        }
        if shared.state.end_requested()
            && shared.state.buffered_bytes() == 0
            && shared.in_flight.is_none()
        {
            shared.finish();
            return std::task::Poll::Ready(None);
        }
        shared.waker = Some(context.waker().clone());
        std::task::Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn response_body_reuses_node_high_water_mark_and_emits_one_drain() {
        let mut body = ResponseBodyDriver::new(8);
        assert_eq!(
            body.write(vec![1; 4]),
            WriteDisposition {
                ready: true,
                submit_now: true,
            }
        );
        assert_eq!(
            body.write(vec![2; 4]),
            WriteDisposition {
                ready: false,
                submit_now: false,
            }
        );
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let mut frames = body.body(sender);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            assert_eq!(
                frames
                    .frame()
                    .await
                    .expect("first frame")
                    .expect("infallible")
                    .into_data()
                    .expect("data frame"),
                vec![1; 4]
            );
            assert_eq!(
                frames
                    .frame()
                    .await
                    .expect("second frame")
                    .expect("infallible")
                    .into_data()
                    .expect("data frame"),
                vec![2; 4]
            );
        });
        assert_eq!(events.try_recv(), Ok(BodyEvent::Drain));
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn end_finishes_only_after_every_ordered_chunk_is_accepted() {
        let mut body = ResponseBodyDriver::new(8);
        assert!(body.end(b"last".to_vec()).ready);
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let mut frames = body.body(sender);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            let data = frames
                .frame()
                .await
                .expect("last frame")
                .expect("infallible")
                .into_data()
                .expect("data frame");
            assert_eq!(data.as_ref(), b"last");
            assert!(frames.frame().await.is_none());
        });
        assert_eq!(events.try_recv(), Ok(BodyEvent::Finish));
    }

    #[test]
    fn a_body_finishes_when_its_last_frame_is_taken_and_never_polled_again() {
        // A response that declares a Content-Length lets Hyper stop the moment it holds the bytes
        // it promised. The body must not wait for the extra poll a chunked encoder would make.
        let mut body = ResponseBodyDriver::new(8);
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let mut frames = body.body(sender);
        assert!(body.end(b"kiln".to_vec()).ready);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            let data = frames
                .frame()
                .await
                .expect("the only frame")
                .expect("infallible")
                .into_data()
                .expect("data frame");
            assert_eq!(data.as_ref(), b"kiln");
        });
        assert_eq!(events.try_recv(), Ok(BodyEvent::Finish));
        assert!(body.finished());
    }

    #[test]
    fn a_response_that_carries_no_body_finishes_without_being_polled() {
        // HEAD requests and bodiless statuses: hyper never asks for a frame at all.
        let mut body = ResponseBodyDriver::new(8);
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let _frames = body.body(sender);
        body.end(b"discarded".to_vec());
        assert!(!body.finished(), "hyper still holds a frame it may take");
        body.finish_without_body();
        assert_eq!(events.try_recv(), Ok(BodyEvent::Finish));
        assert!(body.finished());
    }

    #[test]
    fn empty_end_emits_finish_before_eof() {
        let mut body = ResponseBodyDriver::new(8);
        assert!(body.end(Vec::new()).ready);
        let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
        let mut frames = body.body(sender);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            assert!(frames.frame().await.is_none());
            assert!(frames.frame().await.is_none());
        });
        assert_eq!(events.try_recv(), Ok(BodyEvent::Finish));
        assert!(events.try_recv().is_err());
    }
}

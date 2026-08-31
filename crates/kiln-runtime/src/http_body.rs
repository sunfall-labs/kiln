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

#[derive(Debug, Clone)]
struct Shared {
    state: DuplexState<Vec<u8>>,
    next_submission: u64,
    in_flight: Option<RequestId>,
    finish_emitted: bool,
    waker: Option<Waker>,
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
        self.shared.borrow_mut().state.request_end();
        self.wake();
        disposition
    }

    pub(crate) fn body(
        &self,
        events: tokio::sync::mpsc::UnboundedSender<BodyEvent>,
    ) -> ResponseBody {
        ResponseBody {
            shared: Rc::clone(&self.shared),
            events,
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
    events: tokio::sync::mpsc::UnboundedSender<BodyEvent>,
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
                if event == StreamEvent::Drain {
                    let _ = self.events.send(BodyEvent::Drain);
                }
            }
            if shared.state.end_requested()
                && shared.state.buffered_bytes() == 0
                && !shared.finish_emitted
            {
                shared.finish_emitted = true;
                let _ = self.events.send(BodyEvent::Finish);
            }
        }
        if shared.in_flight.is_none() {
            let request = RequestId::new(shared.next_submission.max(1));
            shared.next_submission = shared.next_submission.wrapping_add(1).max(1);
            if let Some(payload) = shared.state.submit_next(request).cloned() {
                shared.in_flight = Some(request);
                return std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(
                    bytes::Bytes::from(payload),
                ))));
            }
        }
        if shared.state.end_requested()
            && shared.state.buffered_bytes() == 0
            && shared.in_flight.is_none()
        {
            if !shared.finish_emitted {
                shared.finish_emitted = true;
                let _ = self.events.send(BodyEvent::Finish);
            }
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

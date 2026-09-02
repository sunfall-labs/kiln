//! A scripted byte transport for deterministic HTTP runs (ADR 0046).
//!
//! `ScriptedHost` hands one of these to the HTTP engine in place of a socket. Reads replay the
//! recorded inbound chunks, one chunk per poll, and then report EOF or stay pending, as the
//! script says the peer would; writes append to a buffer the test reads back. Nothing here
//! touches a port or the wall clock, so a scripted connection runs the real Hyper future on
//! paused Tokio time and produces the same bytes every run.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The engine's end of a scripted connection.
#[derive(Debug)]
pub struct ScriptedTransport {
    inbound: VecDeque<Vec<u8>>,
    /// Whether the peer closes its write side once the recorded bytes are consumed. When not,
    /// a read past the script stays pending, as a keep-alive socket waiting for the next
    /// request would.
    eof: bool,
    log: ScriptedTransportLog,
}

/// The test's view of what the engine did with the transport.
#[derive(Debug, Clone, Default)]
pub struct ScriptedTransportLog {
    written: Rc<RefCell<Vec<u8>>>,
    shutdown: Rc<Cell<bool>>,
    dropped: Rc<Cell<bool>>,
}

impl ScriptedTransportLog {
    /// Every byte the engine wrote so far.
    pub fn written(&self) -> Vec<u8> {
        self.written.borrow().clone()
    }

    /// The engine shut its write side down.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.get()
    }

    /// The engine dropped the transport: the connection is over.
    pub fn is_dropped(&self) -> bool {
        self.dropped.get()
    }
}

impl ScriptedTransport {
    /// A transport that yields `inbound` one chunk per read and then, if `eof`, reports the
    /// peer's close.
    pub fn new(inbound: impl IntoIterator<Item = Vec<u8>>, eof: bool) -> Self {
        Self {
            inbound: inbound.into_iter().collect(),
            eof,
            log: ScriptedTransportLog::default(),
        }
    }

    pub fn log(&self) -> ScriptedTransportLog {
        self.log.clone()
    }
}

impl Drop for ScriptedTransport {
    fn drop(&mut self) {
        self.log.dropped.set(true);
    }
}

impl AsyncRead for ScriptedTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(mut chunk) = self.inbound.pop_front() else {
            // Nothing else is scripted to arrive. With no peer to wake this read, a pending
            // read stays pending until the engine drops the transport, which is what an open
            // keep-alive socket looks like from Hyper's side.
            return if self.eof {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            };
        };
        let take = chunk.len().min(buffer.remaining());
        buffer.put_slice(&chunk[..take]);
        if take < chunk.len() {
            chunk.drain(..take);
            self.inbound.push_front(chunk);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ScriptedTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.log.written.borrow_mut().extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.log.shutdown.set(true);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn reads_replay_one_chunk_per_poll_and_writes_are_recorded() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            let mut transport = ScriptedTransport::new([b"ab".to_vec(), b"cd".to_vec()], true);
            let log = transport.log();
            let mut buffer = [0u8; 8];
            assert_eq!(transport.read(&mut buffer).await.expect("first"), 2);
            assert_eq!(&buffer[..2], b"ab");
            assert_eq!(transport.read(&mut buffer).await.expect("second"), 2);
            assert_eq!(&buffer[..2], b"cd");
            assert_eq!(transport.read(&mut buffer).await.expect("eof"), 0);
            transport.write_all(b"out").await.expect("writes");
            transport.shutdown().await.expect("shuts down");
            assert_eq!(log.written(), b"out");
            assert!(log.is_shutdown());
            assert!(!log.is_dropped());
            drop(transport);
            assert!(log.is_dropped());
        });
    }

    #[test]
    fn a_chunk_larger_than_the_buffer_is_delivered_across_reads() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime builds");
        runtime.block_on(async {
            let mut transport = ScriptedTransport::new([b"abcdef".to_vec()], true);
            let mut buffer = [0u8; 4];
            assert_eq!(transport.read(&mut buffer).await.expect("first"), 4);
            assert_eq!(&buffer, b"abcd");
            assert_eq!(transport.read(&mut buffer).await.expect("rest"), 2);
            assert_eq!(&buffer[..2], b"ef");
        });
    }
}

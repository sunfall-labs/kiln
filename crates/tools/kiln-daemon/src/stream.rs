//! Delivering a running program's output while it runs.
//!
//! A `Run` or `NativeRun` answers with one summary after the program exits, so the output of a
//! program that never exits -- any server -- reached nobody: `server.listen(port, cb)` called
//! `cb`, `cb` printed "listening", and the line sat in a buffer until the process was killed.
//! A request that sets `stream_output` instead gets [`Response::Output`] frames first, written by
//! whichever transport carries the request (the socket, or the in-process channel of
//! `--one-shot`), and then its one ordinary answer with empty `stdout`/`stderr`.
//!
//! The transport installs the sink for the duration of one request, on the thread that serves
//! it; the code that runs the program asks [`current`] for it. No sink installed means the
//! request did not ask, or nothing is listening, and output is captured into the summary as it
//! always was.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, PoisonError};

use kiln_proto::{OutputStream, Request};

pub(crate) type SinkFn = Box<dyn FnMut(OutputStream, String) + Send>;

/// Shared so the host that executes the program can own a handle to it.
#[derive(Clone)]
pub(crate) struct OutputSink(Arc<Mutex<SinkFn>>);

impl OutputSink {
    pub(crate) fn new(sink: SinkFn) -> Self {
        Self(Arc::new(Mutex::new(sink)))
    }

    pub(crate) fn emit(&self, stream: OutputStream, text: String) {
        if text.is_empty() {
            return;
        }
        let mut sink = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (*sink)(stream, text);
    }
}

thread_local! {
    static CURRENT: RefCell<Option<OutputSink>> = const { RefCell::new(None) };
    static INHERIT_NATIVE: RefCell<bool> = const { RefCell::new(false) };
}

pub(crate) fn inherit_native_output() -> bool {
    INHERIT_NATIVE.with(|value| *value.borrow())
}

pub(crate) struct InheritNativeOutput;

impl Drop for InheritNativeOutput {
    fn drop(&mut self) {
        INHERIT_NATIVE.with(|value| *value.borrow_mut() = false);
    }
}

pub(crate) fn install_inherit_native_output() -> InheritNativeOutput {
    INHERIT_NATIVE.with(|value| *value.borrow_mut() = true);
    InheritNativeOutput
}

/// The sink installed for the request this thread is serving, if it asked for streaming.
pub(crate) fn current() -> Option<OutputSink> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Removes the sink when the request is done, however it ends.
pub(crate) struct Installed;

impl Drop for Installed {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = None);
    }
}

pub(crate) fn install(sink: OutputSink) -> Installed {
    CURRENT.with(|current| *current.borrow_mut() = Some(sink));
    Installed
}

/// Whether `request` asks for its output to be streamed.
pub(crate) fn wants_stream(request: &Request) -> bool {
    matches!(
        request,
        Request::Run {
            stream_output: true,
            ..
        } | Request::NativeRun {
            stream_output: true,
            ..
        }
    )
}

/// Turns raw child-process bytes into text without splitting a UTF-8 sequence across two frames.
#[derive(Default)]
pub(crate) struct Utf8Chunker {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Utf8Chunker {
    pub(crate) fn push(&mut self, stream: OutputStream, bytes: &[u8]) -> String {
        let pending = match stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        };
        pending.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(pending) {
            Ok(_) => pending.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            // Invalid bytes in the middle: take everything, lossily.
            Err(_) => pending.len(),
        };
        let rest = pending.split_off(valid);
        let text = String::from_utf8_lossy(pending).into_owned();
        *pending = rest;
        text
    }

    /// What is left at end of stream: an incomplete tail, lossily decoded.
    pub(crate) fn finish(&mut self) -> [(OutputStream, String); 2] {
        [
            (
                OutputStream::Stdout,
                String::from_utf8_lossy(&std::mem::take(&mut self.stdout)).into_owned(),
            ),
            (
                OutputStream::Stderr,
                String::from_utf8_lossy(&std::mem::take(&mut self.stderr)).into_owned(),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_multibyte_character_is_held_until_it_is_whole() {
        let mut chunker = Utf8Chunker::default();
        let bytes = "é".as_bytes();
        assert_eq!(chunker.push(OutputStream::Stdout, &bytes[..1]), "");
        assert_eq!(chunker.push(OutputStream::Stdout, &bytes[1..]), "é");
        assert_eq!(chunker.push(OutputStream::Stdout, b"ok\n"), "ok\n");
    }
}

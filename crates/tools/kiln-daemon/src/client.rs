//! The client half of the wire protocol.
//!
//! It lives beside the server so the framing rules have exactly one implementation. The CLI is
//! meant to be a thin shell around this: connect, send, print - so that the expensive state
//! stays in the daemon and a command costs a socket round trip rather than a process start.

use crate::framing::{MAX_MESSAGE_BYTES, read_frame, write_frame};
use kiln_proto::{OutputStream, PROTOCOL_VERSION, Request, Response};
use std::io::{self, BufReader, ErrorKind};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect(socket: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(socket)?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    /// Send one request and wait for its response. The protocol is strictly request/response on
    /// a single connection, so responses need no correlation id.
    pub fn request(&mut self, request: &Request) -> io::Result<Response> {
        self.request_streaming(request, &mut |_, _| {})
    }

    /// As [`Self::request`], for a request that sets `stream_output`: the program's output
    /// arrives as [`Response::Output`] frames ahead of the answer, and each goes to `on_output`
    /// as it is read. Still one request and one answer; the frames are the answer in pieces.
    pub fn request_streaming(
        &mut self,
        request: &Request,
        on_output: &mut dyn FnMut(OutputStream, &str),
    ) -> io::Result<Response> {
        let json = serde_json::to_string(request)?;
        write_frame(&mut self.writer, &json)?;

        loop {
            match read_frame(&mut self.reader, MAX_MESSAGE_BYTES)? {
                Some(frame) => match serde_json::from_str(&frame).map_err(io::Error::from)? {
                    Response::Output { stream, text } => on_output(stream, &text),
                    response => return Ok(response),
                },
                None => {
                    return Err(io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "daemon closed the connection without responding",
                    ));
                }
            }
        }
    }

    /// Confirm the daemon speaks this build's protocol before sending anything else.
    ///
    /// Worth doing on every connection: a daemon outlives the binary that started it, so the
    /// common skew is a rebuilt CLI talking to yesterday's daemon.
    pub fn handshake(&mut self) -> io::Result<(u32, u64)> {
        match self.request(&Request::Ping {
            protocol: PROTOCOL_VERSION,
            build: crate::build_identity(),
        })? {
            Response::Pong { pid, build, .. } => Ok((pid, build)),
            Response::Error { message } => Err(io::Error::new(ErrorKind::InvalidData, message)),
            other => Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("expected a Pong, got {other:?}"),
            )),
        }
    }
}

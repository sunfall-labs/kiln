//! Incremental HTTP/1 server semantics for the Node compatibility adapter.
//!
//! This module owns protocol bytes, not sockets or JavaScript objects. A `node:http` adapter
//! feeds it chunks from [`crate::node_stream::StreamDriver`] and turns parsed requests into
//! `IncomingMessage`/`ServerResponse` objects. Keeping protocol and transport separate makes the
//! same parser usable with live Tokio sockets and ADR 0031's scripted deterministic host.

use std::collections::BTreeMap;

use bytes::Bytes;
use kiln_host_abi::{Completion, RequestId, ResourceId};

use crate::node_stream::{
    DEFAULT_HIGH_WATER_MARK, StreamAction, StreamDriver, StreamEvent, WriteDisposition,
};

const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Provider-neutral events produced by one HTTP server connection.
///
/// This is the stable seam between protocol engines and Node adapters. The current incremental
/// decoder and the Hyper migration both produce this vocabulary; neither Tokio nor Hyper types
/// cross it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpConnectionEvent {
    Request(HttpRequestEvent),
    Transport(StreamEvent),
}

/// A protocol failure plus the transport actions required to retire the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConnectionError {
    pub error: HttpParseError,
}

/// Node-facing events and transport work selected by one connection completion.
pub type HttpConnectionTransition =
    Result<(Vec<HttpConnectionEvent>, Vec<StreamAction>), HttpConnectionError>;

/// One HTTP connection composed with Node's provider-neutral duplex stream driver.
///
/// Host operations selected here run unchanged against `TokioHost` and `ScriptedHost`. The
/// decoder is deliberately private so replacing it with a Hyper connection task does not change
/// interpreter, native, or generated-C adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConnectionDriver {
    stream: StreamDriver,
    decoder: Http1Decoder,
}

impl HttpConnectionDriver {
    pub fn new(resource: ResourceId) -> Self {
        Self::with_high_water_mark(resource, DEFAULT_HIGH_WATER_MARK)
    }

    pub fn with_high_water_mark(resource: ResourceId, high_water_mark: usize) -> Self {
        Self {
            stream: StreamDriver::new(resource, high_water_mark),
            decoder: Http1Decoder::default(),
        }
    }

    pub const fn resource(&self) -> ResourceId {
        self.stream.resource()
    }

    pub fn start(&mut self) -> Vec<StreamAction> {
        self.stream.start_reading()
    }

    pub fn started(&mut self, operation: &kiln_host_abi::AsyncHostOp, request: RequestId) -> bool {
        self.stream.started(operation, request)
    }

    pub fn write_response(
        &mut self,
        response: HttpResponse,
        keep_alive: bool,
    ) -> (WriteDisposition, Vec<StreamAction>) {
        self.stream.write(response.encode(keep_alive))
    }

    pub fn write_static_response(
        &mut self,
        content_type: &str,
        body: Bytes,
        keep_alive: bool,
    ) -> (WriteDisposition, Vec<StreamAction>) {
        self.write_response(HttpResponse::static_file(content_type, body), keep_alive)
    }

    pub fn write_response_bytes(
        &mut self,
        response: HttpResponse,
        keep_alive: bool,
    ) -> (WriteDisposition, Vec<StreamAction>) {
        self.stream.write(response.encode(keep_alive))
    }

    /// Apply one realm completion, returning Node-facing events separately from new transport
    /// actions. Protocol failure is reported to the HTTP owner, which must retire the connection
    /// through the runtime HTTP close operation with abort semantics.
    pub fn completed(&mut self, completion: Completion) -> Option<HttpConnectionTransition> {
        let actions = self.stream.completed(completion)?;
        let mut events = Vec::new();
        let mut transport = Vec::new();
        for action in actions {
            match action {
                StreamAction::Data(bytes) => match self.decoder.push_events(&bytes) {
                    Ok(decoded) => {
                        events.extend(decoded.into_iter().map(HttpConnectionEvent::Request))
                    }
                    Err(error) => {
                        return Some(Err(HttpConnectionError { error }));
                    }
                },
                StreamAction::Event(event) => {
                    events.push(HttpConnectionEvent::Transport(event));
                }
                action => transport.push(action),
            }
        }
        Some(Ok((events, transport)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpHeaders {
    raw: Option<Vec<u8>>,
    entries: Vec<(String, String)>,
}

fn find_raw_header<'a>(raw: &'a [u8], name: &str) -> Option<&'a str> {
    let mut at = 0;
    while at < raw.len() {
        let name_start = at;
        while at < raw.len() && raw[at] != 0 {
            at += 1;
        }
        let Ok(key) = std::str::from_utf8(&raw[name_start..at]) else {
            return None;
        };
        at += 1;
        let val_start = at;
        while at < raw.len() && raw[at] != 0 {
            at += 1;
        }
        let Ok(val) = std::str::from_utf8(&raw[val_start..at]) else {
            return None;
        };
        at += 1;
        if key.eq_ignore_ascii_case(name) {
            return Some(val);
        }
    }
    None
}

impl HttpHeaders {
    pub const fn new() -> Self {
        Self {
            raw: None,
            entries: Vec::new(),
        }
    }

    pub fn from_raw(raw: Vec<u8>) -> Self {
        Self {
            raw: Some(raw),
            entries: Vec::new(),
        }
    }

    pub fn from_entries(entries: Vec<(String, String)>) -> Self {
        Self { raw: None, entries }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        if !self.entries.is_empty() {
            return self
                .entries
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str());
        }
        if let Some(raw) = &self.raw {
            return find_raw_header(raw, name);
        }
        None
    }

    pub fn headers_object(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        if !self.entries.is_empty() {
            for (name, value) in &self.entries {
                headers
                    .entry(name.to_ascii_lowercase())
                    .and_modify(|existing: &mut String| {
                        existing.push_str(", ");
                        existing.push_str(value);
                    })
                    .or_insert_with(|| value.clone());
            }
            return headers;
        }
        if let Some(raw) = &self.raw {
            let mut at = 0;
            while at < raw.len() {
                let name_start = at;
                while at < raw.len() && raw[at] != 0 {
                    at += 1;
                }
                let Ok(key) = std::str::from_utf8(&raw[name_start..at]) else {
                    break;
                };
                at += 1;
                let val_start = at;
                while at < raw.len() && raw[at] != 0 {
                    at += 1;
                }
                let Ok(val) = std::str::from_utf8(&raw[val_start..at]) else {
                    break;
                };
                at += 1;
                headers
                    .entry(key.to_ascii_lowercase())
                    .and_modify(|existing: &mut String| {
                        existing.push_str(", ");
                        existing.push_str(val);
                    })
                    .or_insert_with(|| val.to_string());
            }
        }
        headers
    }

    pub fn entries(&mut self) -> &[(String, String)] {
        if self.entries.is_empty()
            && let Some(raw) = &self.raw
        {
            self.entries = crate::unflatten_headers(raw);
        }
        &self.entries
    }
}

impl std::ops::Deref for HttpHeaders {
    type Target = [(String, String)];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl From<Vec<(String, String)>> for HttpHeaders {
    fn from(entries: Vec<(String, String)>) -> Self {
        Self::from_entries(entries)
    }
}

impl From<HttpHeaders> for Vec<(String, String)> {
    fn from(mut headers: HttpHeaders) -> Self {
        headers.entries();
        headers.entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub target: String,
    pub version: HttpVersion,
    pub headers: HttpHeaders,
    pub body: Vec<u8>,
    pub keep_alive: bool,
}

/// One protocol-level transition emitted while decoding an HTTP/1 connection.
///
/// Keeping this below the JavaScript adapter lets live Tokio reads and scripted deterministic
/// reads exercise the same request lifecycle without forcing the decoder to retain whole bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpRequestEvent {
    Head(HttpRequest),
    Data(Vec<u8>),
    End,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.header(name)
    }

    pub fn headers_object(&self) -> BTreeMap<String, String> {
        self.headers.headers_object()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    Http10,
    Http11,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpParseError {
    HeadTooLarge,
    InvalidRequestLine,
    InvalidHeader,
    InvalidContentLength,
    ConflictingBodyFraming,
    InvalidChunk,
    UnsupportedVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyKind {
    None,
    Length(usize),
    Chunked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestHead {
    method: String,
    target: String,
    version: HttpVersion,
    headers: Vec<(String, String)>,
    body: BodyKind,
    keep_alive: bool,
}

/// Incremental, pipelining-safe HTTP/1 request decoder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Http1Decoder {
    bytes: Vec<u8>,
    request: Option<(HttpRequest, BodyKind)>,
}

impl Http1Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<HttpRequest>, HttpParseError> {
        self.bytes.extend_from_slice(bytes);
        let mut requests = Vec::new();
        loop {
            let Some(head_end) = find_bytes(&self.bytes, b"\r\n\r\n") else {
                if self.bytes.len() > MAX_HEAD_BYTES {
                    return Err(HttpParseError::HeadTooLarge);
                }
                break;
            };
            if head_end > MAX_HEAD_BYTES {
                return Err(HttpParseError::HeadTooLarge);
            }
            let head = parse_head(&self.bytes[..head_end])?;
            let body_start = head_end + 4;
            let decoded = match head.body {
                BodyKind::None => Some((Vec::new(), body_start)),
                BodyKind::Length(length) => {
                    let end = body_start.saturating_add(length);
                    (self.bytes.len() >= end).then(|| (self.bytes[body_start..end].to_vec(), end))
                }
                BodyKind::Chunked => decode_chunked(&self.bytes[body_start..])?
                    .map(|(body, consumed)| (body, body_start + consumed)),
            };
            let Some((body, consumed)) = decoded else {
                break;
            };
            requests.push(HttpRequest {
                method: head.method,
                target: head.target,
                version: head.version,
                headers: head.headers.into(),
                body,
                keep_alive: head.keep_alive,
            });
            self.bytes.drain(..consumed);
        }
        Ok(requests)
    }

    /// Decode request lifecycle events without buffering a complete content-length body.
    ///
    /// Chunked framing still uses the complete-frame path for now; content-length requests, the
    /// dominant server upload path, expose every available transport chunk immediately.
    pub fn push_events(&mut self, bytes: &[u8]) -> Result<Vec<HttpRequestEvent>, HttpParseError> {
        self.bytes.extend_from_slice(bytes);
        let mut events = Vec::new();
        loop {
            if self.request.is_none() {
                let Some(head_end) = find_bytes(&self.bytes, b"\r\n\r\n") else {
                    if self.bytes.len() > MAX_HEAD_BYTES {
                        return Err(HttpParseError::HeadTooLarge);
                    }
                    break;
                };
                if head_end > MAX_HEAD_BYTES {
                    return Err(HttpParseError::HeadTooLarge);
                }
                let head = parse_head(&self.bytes[..head_end])?;
                self.bytes.drain(..head_end + 4);
                let request = HttpRequest {
                    method: head.method,
                    target: head.target,
                    version: head.version,
                    headers: head.headers.into(),
                    body: Vec::new(),
                    keep_alive: head.keep_alive,
                };
                let body = head.body;
                events.push(HttpRequestEvent::Head(request.clone()));
                match body {
                    BodyKind::None | BodyKind::Length(0) => {
                        events.push(HttpRequestEvent::End);
                        continue;
                    }
                    _ => self.request = Some((request, body)),
                }
            }

            let Some((_, body)) = self.request.as_mut() else {
                continue;
            };
            match body {
                BodyKind::Length(remaining) => {
                    if self.bytes.is_empty() {
                        break;
                    }
                    let length = (*remaining).min(self.bytes.len());
                    events.push(HttpRequestEvent::Data(self.bytes.drain(..length).collect()));
                    *remaining -= length;
                    if *remaining == 0 {
                        self.request = None;
                        events.push(HttpRequestEvent::End);
                        continue;
                    }
                    break;
                }
                BodyKind::Chunked => {
                    let Some((body, consumed)) = decode_chunked(&self.bytes)? else {
                        break;
                    };
                    self.bytes.drain(..consumed);
                    self.request = None;
                    if !body.is_empty() {
                        events.push(HttpRequestEvent::Data(body));
                    }
                    events.push(HttpRequestEvent::End);
                }
                BodyKind::None => unreachable!("bodyless requests finish with their head"),
            }
        }
        Ok(events)
    }

    pub fn buffered_bytes(&self) -> usize {
        self.bytes.len()
    }
}

fn parse_head(bytes: &[u8]) -> Result<RequestHead, HttpParseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| HttpParseError::InvalidHeader)?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines
        .next()
        .ok_or(HttpParseError::InvalidRequestLine)?
        .split_whitespace();
    let method = request_line
        .next()
        .filter(|value| valid_token(value))
        .ok_or(HttpParseError::InvalidRequestLine)?
        .to_owned();
    let target = request_line
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(HttpParseError::InvalidRequestLine)?
        .to_owned();
    let version = match request_line.next() {
        Some("HTTP/1.0") => HttpVersion::Http10,
        Some("HTTP/1.1") => HttpVersion::Http11,
        Some(_) => return Err(HttpParseError::UnsupportedVersion),
        None => return Err(HttpParseError::InvalidRequestLine),
    };
    if request_line.next().is_some() {
        return Err(HttpParseError::InvalidRequestLine);
    }

    let mut headers = Vec::new();
    let mut content_length = None;
    let mut transfer_codings = Vec::new();
    let mut connection = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(HttpParseError::InvalidHeader)?;
        if !valid_token(name)
            || value
                .bytes()
                .any(|byte| byte == 0 || byte == b'\r' || byte == b'\n')
        {
            return Err(HttpParseError::InvalidHeader);
        }
        let value = value.trim_matches([' ', '\t']).to_owned();
        if name.eq_ignore_ascii_case("content-length") {
            let length = value
                .parse::<usize>()
                .map_err(|_| HttpParseError::InvalidContentLength)?;
            if content_length
                .replace(length)
                .is_some_and(|old| old != length)
            {
                return Err(HttpParseError::InvalidContentLength);
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_codings.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|coding| !coding.is_empty())
                    .map(str::to_ascii_lowercase),
            );
        } else if name.eq_ignore_ascii_case("connection") {
            connection = Some(value.to_ascii_lowercase());
        }
        headers.push((name.to_ascii_lowercase(), value));
    }
    let chunked = !transfer_codings.is_empty()
        && transfer_codings
            .last()
            .is_some_and(|coding| coding == "chunked");
    if !transfer_codings.is_empty() && !chunked {
        return Err(HttpParseError::ConflictingBodyFraming);
    }
    if chunked && content_length.is_some() {
        return Err(HttpParseError::ConflictingBodyFraming);
    }
    let keep_alive = match version {
        HttpVersion::Http11 => connection.as_deref() != Some("close"),
        HttpVersion::Http10 => connection.as_deref() == Some("keep-alive"),
    };
    Ok(RequestHead {
        method,
        target,
        version,
        headers,
        body: if chunked {
            BodyKind::Chunked
        } else if let Some(length) = content_length {
            BodyKind::Length(length)
        } else {
            BodyKind::None
        },
        keep_alive,
    })
}

fn decode_chunked(bytes: &[u8]) -> Result<Option<(Vec<u8>, usize)>, HttpParseError> {
    let mut cursor = 0;
    let mut body = Vec::new();
    loop {
        let Some(line_end) = find_bytes(&bytes[cursor..], b"\r\n") else {
            return Ok(None);
        };
        let line_end = cursor + line_end;
        let size_text = std::str::from_utf8(&bytes[cursor..line_end])
            .map_err(|_| HttpParseError::InvalidChunk)?;
        let size =
            usize::from_str_radix(size_text.split(';').next().unwrap_or_default().trim(), 16)
                .map_err(|_| HttpParseError::InvalidChunk)?;
        cursor = line_end + 2;
        if size == 0 {
            let Some(trailers_end) = find_bytes(&bytes[cursor..], b"\r\n\r\n").or_else(|| {
                bytes
                    .get(cursor..cursor + 2)
                    .filter(|ending| *ending == b"\r\n")
                    .map(|_| 0)
            }) else {
                return Ok(None);
            };
            let consumed = if trailers_end == 0 {
                cursor + 2
            } else {
                cursor + trailers_end + 4
            };
            return Ok(Some((body, consumed)));
        }
        let end = cursor.saturating_add(size);
        if bytes.len() < end + 2 {
            return Ok(None);
        }
        if &bytes[end..end + 2] != b"\r\n" {
            return Err(HttpParseError::InvalidChunk);
        }
        body.extend_from_slice(&bytes[cursor..end]);
        cursor = end + 2;
    }
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

/// One protocol-level transition while decoding an HTTP/1 client response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpResponseEvent {
    Head(HttpResponseHead),
    Data(Vec<u8>),
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponseHead {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub keep_alive: bool,
}

impl HttpResponseHead {
    pub fn headers_object(&self) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        for (name, value) in &self.headers {
            headers
                .entry(name.to_ascii_lowercase())
                .and_modify(|existing: &mut String| {
                    existing.push_str(", ");
                    existing.push_str(value);
                })
                .or_insert_with(|| value.clone());
        }
        headers
    }
}

/// Incremental response decoder used by `node:http` clients.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Http1ResponseDecoder {
    bytes: Vec<u8>,
    body: Option<BodyKind>,
}

impl Http1ResponseDecoder {
    pub fn push_events(&mut self, bytes: &[u8]) -> Result<Vec<HttpResponseEvent>, HttpParseError> {
        self.bytes.extend_from_slice(bytes);
        let mut events = Vec::new();
        if self.body.is_none() {
            let Some(head_end) = find_bytes(&self.bytes, b"\r\n\r\n") else {
                if self.bytes.len() > MAX_HEAD_BYTES {
                    return Err(HttpParseError::HeadTooLarge);
                }
                return Ok(events);
            };
            let (head, body) = parse_response_head(&self.bytes[..head_end])?;
            self.bytes.drain(..head_end + 4);
            events.push(HttpResponseEvent::Head(head));
            match body {
                BodyKind::None | BodyKind::Length(0) => {
                    events.push(HttpResponseEvent::End);
                    return Ok(events);
                }
                body => self.body = Some(body),
            }
        }
        let Some(body) = self.body.as_mut() else {
            return Ok(events);
        };
        match body {
            BodyKind::Length(remaining) => {
                let length = (*remaining).min(self.bytes.len());
                if length != 0 {
                    events.push(HttpResponseEvent::Data(
                        self.bytes.drain(..length).collect(),
                    ));
                    *remaining -= length;
                }
                if *remaining == 0 {
                    self.body = None;
                    events.push(HttpResponseEvent::End);
                }
            }
            BodyKind::Chunked => {
                if let Some((body, consumed)) = decode_chunked(&self.bytes)? {
                    self.bytes.drain(..consumed);
                    self.body = None;
                    if !body.is_empty() {
                        events.push(HttpResponseEvent::Data(body));
                    }
                    events.push(HttpResponseEvent::End);
                }
            }
            BodyKind::None => unreachable!("bodyless response finishes with its head"),
        }
        Ok(events)
    }
}

fn parse_response_head(bytes: &[u8]) -> Result<(HttpResponseHead, BodyKind), HttpParseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| HttpParseError::InvalidHeader)?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().ok_or(HttpParseError::InvalidRequestLine)?;
    let mut parts = status_line.splitn(3, ' ');
    let version = match parts.next() {
        Some("HTTP/1.0") => HttpVersion::Http10,
        Some("HTTP/1.1") => HttpVersion::Http11,
        _ => return Err(HttpParseError::UnsupportedVersion),
    };
    let status = parts
        .next()
        .and_then(|status| status.parse::<u16>().ok())
        .filter(|status| (100..=999).contains(status))
        .ok_or(HttpParseError::InvalidRequestLine)?;
    let reason = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut content_length = None;
    let mut chunked = false;
    let mut connection = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(HttpParseError::InvalidHeader)?;
        if !valid_token(name) {
            return Err(HttpParseError::InvalidHeader);
        }
        let value = value.trim_matches([' ', '\t']).to_owned();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| HttpParseError::InvalidContentLength)?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value
                .split(',')
                .next_back()
                .is_some_and(|coding| coding.trim().eq_ignore_ascii_case("chunked"));
        } else if name.eq_ignore_ascii_case("connection") {
            connection = Some(value.to_ascii_lowercase());
        }
        headers.push((name.to_ascii_lowercase(), value));
    }
    if chunked && content_length.is_some() {
        return Err(HttpParseError::ConflictingBodyFraming);
    }
    let no_body = matches!(status, 100..=199 | 204 | 304);
    let body = if no_body {
        BodyKind::None
    } else if chunked {
        BodyKind::Chunked
    } else if let Some(length) = content_length {
        BodyKind::Length(length)
    } else {
        BodyKind::None
    };
    let keep_alive = match version {
        HttpVersion::Http11 => connection.as_deref() != Some("close"),
        HttpVersion::Http10 => connection.as_deref() == Some("keep-alive"),
    };
    Ok((
        HttpResponseHead {
            status,
            reason,
            headers,
            keep_alive,
        },
        body,
    ))
}

impl HttpResponse {
    pub fn new(status: u16, headers: Vec<(String, String)>, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            reason: None,
            headers,
            body: body.into(),
        }
    }

    /// Build a static file response with direct `Bytes`.
    pub fn static_file(content_type: &str, body: Bytes) -> Self {
        Self {
            status: 200,
            reason: Some("OK".into()),
            headers: vec![
                ("Content-Type".into(), content_type.into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            body,
        }
    }

    pub fn encode_bytes(self, keep_alive: bool) -> Bytes {
        Bytes::from(self.encode(keep_alive))
    }
    /// Encode a response whose body will be written incrementally.
    ///
    /// HTTP/1.1 uses chunked framing when the caller did not provide a length. The adapter owns
    /// chunk boundaries; this method owns only the status line and headers, keeping HTTP bytes
    /// out of the generic stream driver.
    pub fn encode_stream_head(self, keep_alive: bool) -> Vec<u8> {
        let reason = self
            .reason
            .unwrap_or_else(|| reason_phrase(self.status).to_owned());
        let mut headers = self.headers;
        let has_length = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        let has_transfer_encoding = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"));
        let has_connection = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("connection"));
        let no_body = matches!(self.status, 100..=199 | 204 | 304);
        if !has_length && !has_transfer_encoding && !no_body {
            headers.push(("Transfer-Encoding".into(), "chunked".into()));
        }
        if !has_connection && !keep_alive {
            headers.push(("Connection".into(), "close".into()));
        }
        encode_head(self.status, &reason, headers)
    }

    pub fn encode(self, keep_alive: bool) -> Vec<u8> {
        let reason = self
            .reason
            .unwrap_or_else(|| reason_phrase(self.status).to_owned());
        let mut headers = self.headers;
        let has_length = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        let has_connection = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("connection"));
        let no_body = matches!(self.status, 100..=199 | 204 | 304);
        if !has_length && !no_body {
            headers.retain(|(name, _)| !name.eq_ignore_ascii_case("transfer-encoding"));
            headers.push(("Content-Length".into(), self.body.len().to_string()));
        }
        if !has_connection && !keep_alive {
            headers.push(("Connection".into(), "close".into()));
        }
        let mut bytes = encode_head(self.status, &reason, headers);
        if !no_body {
            bytes.extend_from_slice(&self.body);
        }
        bytes
    }
}

fn encode_head(status: u16, reason: &str, headers: Vec<(String, String)>) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {status} {reason}\r\n").into_bytes();
    for (name, value) in headers {
        if valid_token(&name) && !value.contains(['\r', '\n']) {
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(b": ");
            bytes.extend_from_slice(value.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
    }
    bytes.extend_from_slice(b"\r\n");
    bytes
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_incremental_content_length_and_pipelined_requests() {
        let mut decoder = Http1Decoder::default();
        assert!(
            decoder
                .push(b"POST /a HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhe")
                .unwrap()
                .is_empty()
        );
        let requests = decoder
            .push(b"lloGET /b HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, b"hello");
        assert!(requests[0].keep_alive);
        assert_eq!(requests[1].target, "/b");
        assert!(!requests[1].keep_alive);
        assert_eq!(decoder.buffered_bytes(), 0);
    }

    #[test]
    fn event_decoder_streams_content_length_body_across_transport_reads() {
        let mut decoder = Http1Decoder::default();
        assert_eq!(
            decoder
                .push_events(b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhe")
                .unwrap(),
            vec![
                HttpRequestEvent::Head(HttpRequest {
                    method: "POST".into(),
                    target: "/".into(),
                    version: HttpVersion::Http11,
                    headers: vec![("content-length".into(), "5".into())].into(),
                    body: Vec::new(),
                    keep_alive: true,
                }),
                HttpRequestEvent::Data(b"he".to_vec()),
            ]
        );
        assert_eq!(
            decoder
                .push_events(b"lloGET /next HTTP/1.1\r\n\r\n")
                .unwrap(),
            vec![
                HttpRequestEvent::Data(b"llo".to_vec()),
                HttpRequestEvent::End,
                HttpRequestEvent::Head(HttpRequest {
                    method: "GET".into(),
                    target: "/next".into(),
                    version: HttpVersion::Http11,
                    headers: HttpHeaders::new(),
                    body: Vec::new(),
                    keep_alive: true,
                }),
                HttpRequestEvent::End,
            ]
        );
    }

    #[test]
    fn parses_chunked_body_across_input_boundaries() {
        let mut decoder = Http1Decoder::default();
        assert!(
            decoder
                .push(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWi")
                .unwrap()
                .is_empty()
        );
        let request = decoder
            .push(b"ki\r\n5\r\npedia\r\n0\r\n\r\n")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(request.body, b"Wikipedia");
    }

    #[test]
    fn rejects_ambiguous_body_framing() {
        let mut decoder = Http1Decoder::default();
        assert_eq!(
            decoder.push(
                b"POST / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n"
            ),
            Err(HttpParseError::ConflictingBodyFraming)
        );
        let mut decoder = Http1Decoder::default();
        assert_eq!(
            decoder.push(b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n"),
            Err(HttpParseError::ConflictingBodyFraming)
        );
        let mut decoder = Http1Decoder::default();
        assert_eq!(
            decoder.push(b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n"),
            Err(HttpParseError::InvalidContentLength)
        );
    }

    #[test]
    fn response_adds_length_and_connection_policy_without_overwriting_headers() {
        let bytes = HttpResponse {
            status: 200,
            reason: None,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: Bytes::from_static(b"hello"),
        }
        .encode(false);
        assert_eq!(
            bytes,
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
        );
    }

    #[test]
    fn no_content_response_suppresses_a_supplied_body() {
        let bytes = HttpResponse {
            status: 204,
            reason: None,
            headers: Vec::new(),
            body: Bytes::from_static(b"must not be sent"),
        }
        .encode(true);
        assert_eq!(bytes, b"HTTP/1.1 204 No Content\r\n\r\n");
    }

    #[test]
    fn static_file_response_encodes_direct_bytes() {
        let body = Bytes::from_static(b"static payload");
        let resp = HttpResponse::static_file("text/plain", body.clone());
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, body);
        let bytes = resp.encode_bytes(true);
        assert_eq!(
            bytes.as_ref(),
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 14\r\n\r\nstatic payload"
        );
    }

    #[test]
    fn decodes_client_response_across_transport_chunks() {
        let mut decoder = Http1ResponseDecoder::default();
        assert_eq!(
            decoder
                .push_events(b"HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nContent-L")
                .unwrap(),
            []
        );
        let events = decoder
            .push_events(b"ength: 5\r\nConnection: close\r\n\r\nhe")
            .unwrap();
        assert!(matches!(
            &events[0],
            HttpResponseEvent::Head(head)
                if head.status == 201
                    && head.reason == "Created"
                    && head.headers_object().get("content-type").map(String::as_str)
                        == Some("text/plain")
                    && !head.keep_alive
        ));
        assert_eq!(events[1], HttpResponseEvent::Data(b"he".to_vec()));
        assert_eq!(
            decoder.push_events(b"llo").unwrap(),
            [
                HttpResponseEvent::Data(b"llo".to_vec()),
                HttpResponseEvent::End
            ]
        );
    }

    #[test]
    fn chunked_trailers_are_consumed_before_the_next_pipeline_request() {
        let mut decoder = Http1Decoder::default();
        let requests = decoder
            .push(
                b"POST /a HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\nX-T: y\r\n\r\nGET /b HTTP/1.1\r\nHost: x\r\n\r\n",
            )
            .unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, b"x");
        assert_eq!(requests[1].target, "/b");
    }

    fn completion(request: u64, value: kiln_host_abi::AsyncHostValue) -> Completion {
        Completion {
            request: RequestId::new(request),
            sequence: request,
            result: Ok(value),
        }
    }

    #[test]
    fn connection_driver_keeps_transport_actions_below_request_events() {
        use kiln_host_abi::{AsyncHostOp, AsyncHostValue};

        let resource = ResourceId::new(41);
        let mut connection = HttpConnectionDriver::with_high_water_mark(resource, 32);
        let read = connection.start();
        assert_eq!(
            read,
            [StreamAction::Start(AsyncHostOp::TcpRead {
                stream: resource,
                max_bytes: 32,
            })]
        );
        let StreamAction::Start(operation) = &read[0] else {
            unreachable!()
        };
        assert!(connection.started(operation, RequestId::new(1)));

        let (events, actions) = connection
            .completed(completion(
                1,
                AsyncHostValue::Bytes(
                    b"POST /items HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc".to_vec(),
                ),
            ))
            .expect("completion belongs to connection")
            .expect("request parses");
        assert!(matches!(
            &events[..],
            [
                HttpConnectionEvent::Request(HttpRequestEvent::Head(HttpRequest {
                    method,
                    target,
                    ..
                })),
                HttpConnectionEvent::Request(HttpRequestEvent::Data(data)),
                HttpConnectionEvent::Request(HttpRequestEvent::End),
            ] if method == "POST" && target == "/items" && data == b"abc"
        ));
        assert_eq!(
            actions,
            [StreamAction::Start(AsyncHostOp::TcpRead {
                stream: resource,
                max_bytes: 32,
            })]
        );
    }

    #[test]
    fn connection_driver_reports_protocol_failure_without_raw_close() {
        use kiln_host_abi::AsyncHostValue;

        let resource = ResourceId::new(42);
        let mut connection = HttpConnectionDriver::new(resource);
        let read = connection.start();
        let StreamAction::Start(operation) = &read[0] else {
            unreachable!()
        };
        assert!(connection.started(operation, RequestId::new(1)));
        assert_eq!(
            connection.completed(completion(
                1,
                AsyncHostValue::Bytes(
                    b"POST / HTTP/1.1\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n"
                        .to_vec(),
                ),
            )),
            Some(Err(HttpConnectionError {
                error: HttpParseError::ConflictingBodyFraming,
            }))
        );
    }
}

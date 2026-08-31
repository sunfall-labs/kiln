//! Hyper-backed HTTP protocol engine behind Kiln's runtime-private seam.
//!
//! This module deliberately contains every Hyper type used by the runtime. Node adapters consume
//! `node_http` events and owned bytes; HostOps consumes portable resource operations. Keeping this
//! module private prevents either external seam from inheriting Hyper's API or release cadence.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use http_body_util::BodyExt;
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::http_body::ResponseBody;
use crate::node_http::{HttpRequest, HttpVersion};

#[derive(Debug)]
pub(crate) enum EngineEvent {
    RequestHead { id: u64, request: HttpRequest },
    RequestData { id: u64, data: Vec<u8> },
    RequestEnd { id: u64 },
    Error { id: u64, message: String },
    Closed,
}

pub(crate) struct ResponseCommand {
    pub(crate) id: u64,
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: ResponseBody,
}

pub(crate) struct ClientRequestCommand {
    pub(crate) id: u64,
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: ResponseBody,
}

#[derive(Debug)]
pub(crate) enum ClientEvent {
    Ready {
        id: u64,
    },
    ResponseHead {
        id: u64,
        status: u16,
        reason: String,
        headers: Vec<(String, String)>,
    },
    ResponseData {
        id: u64,
        data: Vec<u8>,
    },
    ResponseEnd {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    Closed,
}

pub(crate) fn client_connection<I>(
    io: I,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<ClientRequestCommand>,
    events: tokio::sync::mpsc::UnboundedSender<ClientEvent>,
) -> ConnectionFuture
where
    I: AsyncRead + AsyncWrite + Unpin + 'static,
{
    Box::pin(async move {
        let (mut sender, connection) =
            match hyper::client::conn::http1::handshake(TokioIo::new(io)).await {
                Ok(parts) => parts,
                Err(error) => {
                    let _ = events.send(ClientEvent::Error {
                        id: 0,
                        message: error.to_string(),
                    });
                    let _ = events.send(ClientEvent::Closed);
                    return Ok(());
                }
            };
        let mut connection_task = tokio::task::spawn_local(connection);
        loop {
            let command = tokio::select! {
                biased;
                command = requests.recv() => command,
                result = &mut connection_task => {
                    match result {
                        Ok(Err(error)) => {
                            let _ = events.send(ClientEvent::Error { id: 0, message: error.to_string() });
                        }
                        Err(error) => {
                            let _ = events.send(ClientEvent::Error { id: 0, message: error.to_string() });
                        }
                        Ok(Ok(())) => {}
                    }
                    let _ = events.send(ClientEvent::Closed);
                    return Ok(());
                }
            };
            let Some(command) = command else { break };
            let id = command.id;
            let mut builder = Request::builder()
                .method(command.method.as_str())
                .uri(command.target.as_str());
            for (name, value) in command.headers {
                builder = builder.header(name, value);
            }
            let request = match builder.body(command.body) {
                Ok(request) => request,
                Err(error) => {
                    connection_task.abort();
                    let _ = events.send(ClientEvent::Error {
                        id,
                        message: error.to_string(),
                    });
                    let _ = events.send(ClientEvent::Closed);
                    return Ok(());
                }
            };
            if let Err(error) = sender.ready().await {
                connection_task.abort();
                let _ = events.send(ClientEvent::Error {
                    id,
                    message: error.to_string(),
                });
                let _ = events.send(ClientEvent::Closed);
                return Ok(());
            }
            match sender.send_request(request).await {
                Ok(mut response) => {
                    let status = response.status();
                    let headers = response
                        .headers()
                        .iter()
                        .filter_map(|(name, value)| {
                            value
                                .to_str()
                                .ok()
                                .map(|value| (name.as_str().to_owned(), value.to_owned()))
                        })
                        .collect();
                    let _ = events.send(ClientEvent::ResponseHead {
                        id,
                        status: status.as_u16(),
                        reason: status.canonical_reason().unwrap_or_default().to_owned(),
                        headers,
                    });
                    while let Some(frame) = response.body_mut().frame().await {
                        let frame = match frame {
                            Ok(frame) => frame,
                            Err(error) => {
                                connection_task.abort();
                                let _ = events.send(ClientEvent::Error {
                                    id,
                                    message: error.to_string(),
                                });
                                let _ = events.send(ClientEvent::Closed);
                                return Ok(());
                            }
                        };
                        if let Ok(data) = frame.into_data()
                            && !data.is_empty()
                        {
                            let _ = events.send(ClientEvent::ResponseData {
                                id,
                                data: data.to_vec(),
                            });
                        }
                    }
                    let _ = events.send(ClientEvent::ResponseEnd { id });
                    if let Err(error) = sender.ready().await {
                        connection_task.abort();
                        let _ = events.send(ClientEvent::Error {
                            id,
                            message: error.to_string(),
                        });
                        let _ = events.send(ClientEvent::Closed);
                        return Ok(());
                    }
                    let _ = events.send(ClientEvent::Ready { id });
                }
                Err(error) => {
                    connection_task.abort();
                    let _ = events.send(ClientEvent::Error {
                        id,
                        message: error.to_string(),
                    });
                    let _ = events.send(ClientEvent::Closed);
                    return Ok(());
                }
            }
        }
        drop(sender);
        connection_task.abort();
        let _ = events.send(ClientEvent::Closed);
        Ok(())
    })
}

async fn serve_http1_service<I, F, Fut>(
    io: I,
    allow_half_close: bool,
    service: F,
) -> Result<(), hyper::Error>
where
    I: AsyncRead + AsyncWrite + Unpin,
    F: Fn(Request<Incoming>) -> Fut,
    Fut: Future<Output = Result<Response<ResponseBody>, std::convert::Infallible>>,
{
    let service = service_fn(service);
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.auto_date_header(false);
    builder.half_close(allow_half_close);
    builder.serve_connection(TokioIo::new(io), service).await
}

pub(crate) type ConnectionFuture =
    Pin<Box<dyn Future<Output = Result<(), hyper::Error>> + 'static>>;

/// Build one connection future whose protocol transitions cross only Kiln-owned channels.
/// JavaScript is never called by Hyper; the realm consumes `EngineEvent` at its normal checkpoint
/// and sends a response command after the request listener settles.
pub(crate) fn event_connection<I>(
    io: I,
    allow_half_close: bool,
    events: tokio::sync::mpsc::UnboundedSender<EngineEvent>,
    responses: tokio::sync::mpsc::UnboundedReceiver<ResponseCommand>,
) -> ConnectionFuture
where
    I: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let responses = std::rc::Rc::new(tokio::sync::Mutex::new((
        responses,
        BTreeMap::<u64, ResponseCommand>::new(),
    )));
    let next_id = std::rc::Rc::new(std::cell::Cell::new(1_u64));
    Box::pin(async move {
        let service_events = events.clone();
        let result = serve_http1_service(io, allow_half_close, move |request| {
            let id = next_id.get();
            next_id.set(id.wrapping_add(1).max(1));
            let events = service_events.clone();
            let responses = std::rc::Rc::clone(&responses);
            async move {
                let (parts, mut body) = request.into_parts();
                let request = kiln_request_parts(&parts);
                let keep_alive = request.keep_alive;
                let _ = events.send(EngineEvent::RequestHead { id, request });
                while let Some(frame) = body.frame().await {
                    match frame {
                        Ok(frame) => {
                            if let Ok(data) = frame.into_data()
                                && !data.is_empty()
                            {
                                let _ = events.send(EngineEvent::RequestData {
                                    id,
                                    data: data.to_vec(),
                                });
                            }
                        }
                        Err(error) => {
                            let _ = events.send(EngineEvent::Error {
                                id,
                                message: error.to_string(),
                            });
                            break;
                        }
                    }
                }
                let _ = events.send(EngineEvent::RequestEnd { id });
                let mut response = loop {
                    let mut responses = responses.lock().await;
                    if let Some(command) = responses.1.remove(&id) {
                        break command;
                    }
                    match responses.0.recv().await {
                        Some(command) if command.id == id => break command,
                        Some(command) => {
                            responses.1.insert(command.id, command);
                        }
                        None => {
                            let mut driver = crate::http_body::ResponseBodyDriver::default();
                            driver.end(Vec::new());
                            let (event_sender, _) = tokio::sync::mpsc::unbounded_channel();
                            break ResponseCommand {
                                id,
                                status: 503,
                                headers: vec![("connection".into(), "close".into())],
                                body: driver.body(event_sender),
                            };
                        }
                    }
                };
                if !keep_alive
                    && !response
                        .headers
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case("connection"))
                {
                    response.headers.push(("connection".into(), "close".into()));
                }
                Ok::<_, std::convert::Infallible>(hyper_response(response))
            }
        })
        .await;
        if let Err(error) = &result {
            let _ = events.send(EngineEvent::Error {
                id: 0,
                message: error.to_string(),
            });
        }
        let _ = events.send(EngineEvent::Closed);
        result
    })
}

fn kiln_request_parts(parts: &hyper::http::request::Parts) -> HttpRequest {
    let version = match parts.version {
        hyper::Version::HTTP_10 => HttpVersion::Http10,
        _ => HttpVersion::Http11,
    };
    let keep_alive = !parts
        .headers
        .get(hyper::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("close"));
    HttpRequest {
        method: parts.method.as_str().to_owned(),
        target: parts.uri.to_string(),
        version,
        headers: parts
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect(),
        body: Vec::new(),
        keep_alive,
    }
}

fn hyper_response(response: ResponseCommand) -> Response<ResponseBody> {
    let mut builder = Response::builder().status(response.status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(response.body)
        .expect("Kiln validated the HTTP response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_transport;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::client::conn::http1;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    #[test]
    fn hyper_http1_runs_on_the_owned_current_thread_runtime_without_ambient_date() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Tokio runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (client_io, server_io) = duplex(4096);
            let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                server_io,
                false,
                event_sender,
                response_receiver,
            ));

            let (mut sender, connection) = http1::handshake(TokioIo::new(client_io))
                .await
                .expect("client handshake succeeds");
            let client = tokio::task::spawn_local(async move {
                connection.await.expect("client connection completes")
            });
            let response_future = sender.send_request(
                Request::builder()
                    .uri("/health")
                    .body(Full::<Bytes>::default())
                    .expect("request is valid"),
            );
            let event = event_receiver.recv().await.expect("request head arrives");
            let EngineEvent::RequestHead { id, request } = event else {
                panic!("expected request head")
            };
            assert_eq!(request.method, "GET");
            assert_eq!(request.target, "/health");
            assert!(matches!(
                event_receiver.recv().await,
                Some(EngineEvent::RequestEnd { id: end_id }) if end_id == id
            ));
            let mut body = crate::http_body::ResponseBodyDriver::default();
            body.end(b"kiln".to_vec());
            let (body_event_sender, _) = tokio::sync::mpsc::unbounded_channel();
            response_sender
                .send(ResponseCommand {
                    id,
                    status: 200,
                    headers: vec![("content-type".into(), "text/plain".into())],
                    body: body.body(body_event_sender),
                })
                .expect("response command sends");
            let response = response_future.await.expect("request succeeds");
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "text/plain");
            assert!(!response.headers().contains_key("date"));
            assert_eq!(
                response
                    .into_body()
                    .collect()
                    .await
                    .expect("body reads")
                    .to_bytes(),
                "kiln"
            );
            drop(sender);
            client.await.expect("client task joins");
            server
                .await
                .expect("server task joins")
                .expect("server connection completes");
        });
    }

    #[test]
    fn hyper_http1_runs_over_the_realm_completion_transport() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (io, mut realm) = http_transport::channel();
            let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                io,
                false,
                event_sender,
                response_receiver,
            ));

            realm.feed_read(b"GET / HTTP/1.1\r\nhost: kiln\r\n\r\n".to_vec());
            tokio::task::yield_now().await;
            let EngineEvent::RequestHead { id, .. } =
                event_receiver.recv().await.expect("request event arrives")
            else {
                panic!("expected request event")
            };
            assert!(matches!(
                event_receiver.recv().await,
                Some(EngineEvent::RequestEnd { id: end_id }) if end_id == id
            ));
            let mut body = crate::http_body::ResponseBodyDriver::default();
            body.end(b"kiln".to_vec());
            let (body_event_sender, _) = tokio::sync::mpsc::unbounded_channel();
            response_sender
                .send(ResponseCommand {
                    id,
                    status: 200,
                    headers: vec![("connection".into(), "close".into())],
                    body: body.body(body_event_sender),
                })
                .expect("response command sends");
            let mut response = Vec::new();
            loop {
                tokio::task::yield_now().await;
                while let Some(bytes) = realm.next_write() {
                    response.extend_from_slice(&bytes);
                    assert!(realm.acknowledge_write());
                }
                if server.is_finished() {
                    break;
                }
            }
            server
                .await
                .expect("server task joins")
                .expect("server connection completes");
            let response = String::from_utf8(response).expect("HTTP is ASCII");
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(
                response
                    .to_ascii_lowercase()
                    .contains("transfer-encoding: chunked\r\n")
            );
            assert!(response.ends_with("4\r\nkiln\r\n0\r\n\r\n"));
            assert!(!response.to_ascii_lowercase().contains("\r\ndate:"));
        });
    }

    #[test]
    fn client_request_body_streams_ordered_chunks_before_end() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (client_io, server_io) = duplex(4096);
            let (request_sender, request_receiver) = tokio::sync::mpsc::unbounded_channel();
            let (client_event_sender, mut client_events) = tokio::sync::mpsc::unbounded_channel();
            let client = tokio::task::spawn_local(client_connection(
                client_io,
                request_receiver,
                client_event_sender,
            ));

            let (server_event_sender, mut server_events) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                server_io,
                false,
                server_event_sender,
                response_receiver,
            ));

            let mut body = crate::http_body::ResponseBodyDriver::new(8);
            let (body_event_sender, _body_events) = tokio::sync::mpsc::unbounded_channel();
            request_sender
                .send(ClientRequestCommand {
                    id: 1,
                    method: "POST".into(),
                    target: "/upload".into(),
                    headers: vec![("content-type".into(), "text/plain".into())],
                    body: body.body(body_event_sender),
                })
                .expect("request start sends");

            assert!(body.write(b"kiln ".to_vec()).ready);
            let EngineEvent::RequestHead { id, request } =
                server_events.recv().await.expect("request head arrives")
            else {
                panic!("expected request head")
            };
            assert_eq!(request.method, "POST");
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(name, value)| { name == "content-type" && value == "text/plain" })
            );
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(name, value)| { name == "transfer-encoding" && value == "chunked" })
            );
            assert!(matches!(
                server_events.recv().await,
                Some(EngineEvent::RequestData { id: data_id, data })
                    if data_id == id && data == b"kiln "
            ));

            assert!(body.write(b"streams".to_vec()).ready);
            assert!(matches!(
                server_events.recv().await,
                Some(EngineEvent::RequestData { id: data_id, data })
                    if data_id == id && data == b"streams"
            ));
            body.end(Vec::new());
            assert!(matches!(
                server_events.recv().await,
                Some(EngineEvent::RequestEnd { id: end_id }) if end_id == id
            ));
            let mut response_body = crate::http_body::ResponseBodyDriver::default();
            response_body.end(b"ok".to_vec());
            let (response_body_events, _) = tokio::sync::mpsc::unbounded_channel();
            response_sender
                .send(ResponseCommand {
                    id,
                    status: 200,
                    headers: vec![("connection".into(), "close".into())],
                    body: response_body.body(response_body_events),
                })
                .expect("response sends");
            assert!(matches!(
                client_events.recv().await,
                Some(ClientEvent::ResponseHead { status: 200, .. })
            ));
            assert!(matches!(
                client_events.recv().await,
                Some(ClientEvent::ResponseData { id: 1, data }) if data == b"ok"
            ));
            assert!(matches!(
                client_events.recv().await,
                Some(ClientEvent::ResponseEnd { id: 1 })
            ));
            drop(request_sender);
            client
                .await
                .expect("client task joins")
                .expect("client succeeds");
            server
                .await
                .expect("server task joins")
                .expect("server succeeds");
        });
    }

    #[test]
    fn client_connection_reuses_one_http1_socket_for_sequential_requests() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("Tokio runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (client_io, server_io) = duplex(8192);
            let (request_sender, request_receiver) = tokio::sync::mpsc::unbounded_channel();
            let (client_event_sender, mut client_events) = tokio::sync::mpsc::unbounded_channel();
            let client = tokio::task::spawn_local(client_connection(
                client_io,
                request_receiver,
                client_event_sender,
            ));
            let (server_event_sender, mut server_events) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                server_io,
                false,
                server_event_sender,
                response_receiver,
            ));

            for request_id in 1..=2 {
                let mut body = crate::http_body::ResponseBodyDriver::default();
                body.end(Vec::new());
                let (body_events, _) = tokio::sync::mpsc::unbounded_channel();
                request_sender
                    .send(ClientRequestCommand {
                        id: request_id,
                        method: "GET".into(),
                        target: format!("/{request_id}"),
                        headers: vec![("host".into(), "kiln".into())],
                        body: body.body(body_events),
                    })
                    .expect("request command sends");
                let EngineEvent::RequestHead { id, request } =
                    server_events.recv().await.expect("request head arrives")
                else {
                    panic!("expected request head")
                };
                assert_eq!(request.target, format!("/{request_id}"));
                assert!(matches!(
                    server_events.recv().await,
                    Some(EngineEvent::RequestEnd { id: end_id }) if end_id == id
                ));
                let mut response_body = crate::http_body::ResponseBodyDriver::default();
                response_body.end(request_id.to_string().into_bytes());
                let (response_events, _) = tokio::sync::mpsc::unbounded_channel();
                response_sender
                    .send(ResponseCommand {
                        id,
                        status: 200,
                        headers: Vec::new(),
                        body: response_body.body(response_events),
                    })
                    .expect("response sends");
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::ResponseHead { id, status: 200, .. }) if id == request_id
                ));
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::ResponseData { id, data })
                        if id == request_id && data == request_id.to_string().as_bytes()
                ));
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::ResponseEnd { id }) if id == request_id
                ));
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::Ready { id }) if id == request_id
                ));
            }

            drop(request_sender);
            client
                .await
                .expect("client task joins")
                .expect("client succeeds");
            assert!(matches!(
                client_events.recv().await,
                Some(ClientEvent::Closed)
            ));
            server
                .await
                .expect("server task joins")
                .expect("server succeeds");
        });
    }

    #[test]
    fn server_connection_dispatches_the_next_pipeline_request_after_the_prior_head() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("Tokio runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (mut client_io, server_io) = duplex(8192);
            let (event_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                server_io,
                false,
                event_sender,
                response_receiver,
            ));

            client_io
                .write_all(
                    b"GET /first HTTP/1.1\r\nHost: kiln\r\n\r\nGET /second HTTP/1.1\r\nHost: kiln\r\n\r\n",
                )
                .await
                .expect("pipelined requests write");

            let first_id = loop {
                match events.recv().await.expect("first server event arrives") {
                    EngineEvent::RequestHead { id, request } => {
                        assert_eq!(request.target, "/first");
                        break id;
                    }
                    event => panic!("unexpected server event before first head: {event:?}"),
                }
            };
            assert!(matches!(
                events.recv().await,
                Some(EngineEvent::RequestEnd { id }) if id == first_id
            ));

            let mut response_body = crate::http_body::ResponseBodyDriver::default();
            let (body_events, _) = tokio::sync::mpsc::unbounded_channel();
            response_sender
                .send(ResponseCommand {
                    id: first_id,
                    status: 200,
                    headers: vec![("content-length".into(), "5".into())],
                    body: response_body.body(body_events),
                })
                .expect("first response command sends");
            response_body.end(b"first".to_vec());

            let second_id = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    match events.recv().await.expect("second server event arrives") {
                        EngineEvent::RequestHead { id, request } => {
                            assert_eq!(request.target, "/second");
                            break id;
                        }
                        event => panic!("unexpected server event before second head: {event:?}"),
                    }
                }
            })
            .await
            .expect("second pipelined request head arrives after the first response head");

            for (id, body) in [(second_id, b"second".as_slice())] {
                let mut response_body = crate::http_body::ResponseBodyDriver::default();
                response_body.end(body.to_vec());
                let (body_events, _) = tokio::sync::mpsc::unbounded_channel();
                response_sender
                    .send(ResponseCommand {
                        id,
                        status: 200,
                        headers: vec![("content-length".into(), body.len().to_string())],
                        body: response_body.body(body_events),
                    })
                    .expect("response command sends");
            }

            let response = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                let mut response = Vec::new();
                let mut chunk = [0; 512];
                while !response.windows(b"second".len()).any(|bytes| bytes == b"second") {
                    let length = client_io.read(&mut chunk).await.expect("response reads");
                    assert_ne!(length, 0, "connection closed before the second response");
                    response.extend_from_slice(&chunk[..length]);
                }
                response
            })
            .await
            .expect("both pipelined response bodies arrive");
            let response = String::from_utf8(response).expect("HTTP is ASCII");
            assert!(
                response.find("first").is_some_and(|first| {
                    response
                        .find("second")
                        .is_some_and(|second| first < second)
                }),
                "responses were not serialized in request order: {response:?}"
            );

            drop(response_sender);
            drop(client_io);
            server.abort();
        });
    }

    #[test]
    fn server_connection_allows_response_after_client_half_close_when_enabled() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("Tokio runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (mut client_io, server_io) = duplex(4096);
            let (event_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                server_io,
                true,
                event_sender,
                response_receiver,
            ));
            client_io
                .write_all(b"GET /half HTTP/1.1\r\nHost: kiln\r\nConnection: close\r\n\r\n")
                .await
                .expect("request writes");
            client_io.shutdown().await.expect("client half closes");

            let id = match events.recv().await.expect("request event arrives") {
                EngineEvent::RequestHead { id, request } => {
                    assert_eq!(request.target, "/half");
                    id
                }
                event => panic!("unexpected event before request head: {event:?}"),
            };
            assert!(matches!(
                events.recv().await,
                Some(EngineEvent::RequestEnd { id: end_id }) if end_id == id
            ));
            let mut body = crate::http_body::ResponseBodyDriver::default();
            body.end(b"half-open".to_vec());
            let (body_events, _) = tokio::sync::mpsc::unbounded_channel();
            response_sender
                .send(ResponseCommand {
                    id,
                    status: 200,
                    headers: vec![("content-length".into(), "9".into())],
                    body: body.body(body_events),
                })
                .expect("response sends");

            let mut response = Vec::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                client_io.read_to_end(&mut response),
            )
            .await
            .expect("response completes after half close")
            .expect("response reads");
            let response = String::from_utf8(response).expect("HTTP is ASCII");
            assert!(response.ends_with("half-open"), "response was {response:?}");
            server
                .await
                .expect("server task joins")
                .expect("server connection succeeds");
        });
    }

    #[test]
    fn client_connection_reuses_one_real_tcp_socket_for_sequential_requests() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("listener binds");
            let address = listener.local_addr().expect("listener has an address");
            let (request_sender, request_receiver) = tokio::sync::mpsc::unbounded_channel();
            let (client_event_sender, mut client_events) = tokio::sync::mpsc::unbounded_channel();
            let client_io = tokio::net::TcpStream::connect(address)
                .await
                .expect("client connects");
            let client = tokio::task::spawn_local(client_connection(
                client_io,
                request_receiver,
                client_event_sender,
            ));
            let (server_io, _) = listener.accept().await.expect("server accepts");
            let (server_event_sender, mut server_events) = tokio::sync::mpsc::unbounded_channel();
            let (response_sender, response_receiver) = tokio::sync::mpsc::unbounded_channel();
            let server = tokio::task::spawn_local(event_connection(
                server_io,
                false,
                server_event_sender,
                response_receiver,
            ));

            for request_id in 1..=2 {
                let mut body = crate::http_body::ResponseBodyDriver::default();
                body.end(Vec::new());
                let (body_events, _) = tokio::sync::mpsc::unbounded_channel();
                request_sender
                    .send(ClientRequestCommand {
                        id: request_id,
                        method: "GET".into(),
                        target: format!("/{request_id}"),
                        headers: vec![("host".into(), "kiln".into())],
                        body: body.body(body_events),
                    })
                    .expect("request command sends");
                let EngineEvent::RequestHead { id, request } =
                    server_events.recv().await.expect("request head arrives")
                else {
                    panic!("expected request head")
                };
                assert_eq!(request.target, format!("/{request_id}"));
                assert!(matches!(
                    server_events.recv().await,
                    Some(EngineEvent::RequestEnd { id: end_id }) if end_id == id
                ));
                let mut response_body = crate::http_body::ResponseBodyDriver::default();
                response_body.end(request_id.to_string().into_bytes());
                let (response_events, _) = tokio::sync::mpsc::unbounded_channel();
                response_sender
                    .send(ResponseCommand {
                        id,
                        status: 200,
                        headers: Vec::new(),
                        body: response_body.body(response_events),
                    })
                    .expect("response sends");
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::ResponseHead { id, status: 200, .. }) if id == request_id
                ));
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::ResponseData { id, data })
                        if id == request_id && data == request_id.to_string().as_bytes()
                ));
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::ResponseEnd { id }) if id == request_id
                ));
                assert!(matches!(
                    client_events.recv().await,
                    Some(ClientEvent::Ready { id }) if id == request_id
                ));
            }
            drop(request_sender);
            client
                .await
                .expect("client task joins")
                .expect("client succeeds");
            server
                .await
                .expect("server task joins")
                .expect("server succeeds");
        });
    }

    #[test]
    fn client_connection_reports_task_failure_with_terminal_events() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime builds");
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async {
            let (client_io, server_io) = duplex(1024);
            let (request_sender, request_receiver) = tokio::sync::mpsc::unbounded_channel();
            let (event_sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            let client = tokio::task::spawn_local(client_connection(
                client_io,
                request_receiver,
                event_sender,
            ));
            drop(server_io);
            assert!(matches!(
                events.recv().await,
                Some(ClientEvent::Error { id: 0, .. })
            ));
            assert!(matches!(events.recv().await, Some(ClientEvent::Closed)));
            drop(request_sender);
            client
                .await
                .expect("client task joins")
                .expect("client succeeds");
        });
    }
}

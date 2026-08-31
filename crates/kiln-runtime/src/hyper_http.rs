//! Hyper-backed HTTP protocol engine behind Kiln's runtime-private seam.
//!
//! This module deliberately contains every Hyper type used by the runtime. Node adapters consume
//! `node_http` events and owned bytes; HostOps consumes portable resource operations. Keeping this
//! module private prevents either external seam from inheriting Hyper's API or release cadence.

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
    Error(String),
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
    Error(String),
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
            hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
        let connection_task = tokio::task::spawn_local(connection);
        while let Some(command) = requests.recv().await {
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
                    let _ = events.send(ClientEvent::Error(error.to_string()));
                    return Ok(());
                }
            };
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
                        let frame = frame?;
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
                }
                Err(error) => {
                    let _ = events.send(ClientEvent::Error(error.to_string()));
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

async fn serve_http1_service<I, F, Fut>(io: I, service: F) -> Result<(), hyper::Error>
where
    I: AsyncRead + AsyncWrite + Unpin,
    F: Fn(Request<Incoming>) -> Fut,
    Fut: Future<Output = Result<Response<ResponseBody>, std::convert::Infallible>>,
{
    let service = service_fn(service);
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.auto_date_header(false);
    builder.serve_connection(TokioIo::new(io), service).await
}

pub(crate) type ConnectionFuture =
    Pin<Box<dyn Future<Output = Result<(), hyper::Error>> + 'static>>;

/// Build one connection future whose protocol transitions cross only Kiln-owned channels.
/// JavaScript is never called by Hyper; the realm consumes `EngineEvent` at its normal checkpoint
/// and sends a response command after the request listener settles.
pub(crate) fn event_connection<I>(
    io: I,
    events: tokio::sync::mpsc::UnboundedSender<EngineEvent>,
    responses: tokio::sync::mpsc::UnboundedReceiver<ResponseCommand>,
) -> ConnectionFuture
where
    I: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let responses = std::rc::Rc::new(tokio::sync::Mutex::new(responses));
    let next_id = std::rc::Rc::new(std::cell::Cell::new(1_u64));
    Box::pin(async move {
        let service_events = events.clone();
        let result = serve_http1_service(io, move |request| {
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
                            let _ = events.send(EngineEvent::Error(error.to_string()));
                            break;
                        }
                    }
                }
                let _ = events.send(EngineEvent::RequestEnd { id });
                let mut response = loop {
                    match responses.lock().await.recv().await {
                        Some(command) if command.id == id => break command,
                        Some(_) => continue,
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
        let _ = events.send(match &result {
            Ok(()) => EngineEvent::Closed,
            Err(error) => EngineEvent::Error(error.to_string()),
        });
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
    use tokio::io::duplex;

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
            let server =
                tokio::task::spawn_local(event_connection(io, event_sender, response_receiver));

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
                assert!(client_events.try_recv().is_err());
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
}

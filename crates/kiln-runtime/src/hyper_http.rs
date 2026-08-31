//! Hyper-backed HTTP protocol engine behind Kiln's runtime-private seam.
//!
//! This module deliberately contains every Hyper type used by the runtime. Node adapters consume
//! `node_http` events and owned bytes; HostOps consumes portable resource operations. Keeping this
//! module private prevents either external seam from inheriting Hyper's API or release cadence.

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::node_http::{HttpRequest, HttpResponse, HttpVersion};

#[derive(Debug)]
pub(crate) enum EngineEvent {
    Request {
        id: u64,
        request: HttpRequest,
        body: Vec<u8>,
    },
    Error(String),
    Closed,
}

pub(crate) struct ResponseCommand {
    pub(crate) id: u64,
    pub(crate) response: HttpResponse,
}

async fn serve_http1_service<I, F, Fut>(io: I, service: F) -> Result<(), hyper::Error>
where
    I: AsyncRead + AsyncWrite + Unpin,
    F: Fn(Request<Incoming>) -> Fut,
    Fut: Future<Output = Result<Response<Full<Bytes>>, std::convert::Infallible>>,
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
                let (parts, body) = request.into_parts();
                let body = body
                    .collect()
                    .await
                    .map(|body| body.to_bytes().to_vec())
                    .unwrap_or_default();
                let request = kiln_request_parts(&parts);
                let _ = events.send(EngineEvent::Request { id, request, body });
                let response = loop {
                    match responses.lock().await.recv().await {
                        Some(command) if command.id == id => break command.response,
                        Some(_) => continue,
                        None => {
                            break HttpResponse {
                                status: 503,
                                reason: None,
                                headers: vec![("connection".into(), "close".into())],
                                body: Vec::new(),
                            };
                        }
                    }
                };
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

fn hyper_response(response: HttpResponse) -> Response<Full<Bytes>> {
    let mut builder = Response::builder().status(response.status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(Full::new(Bytes::from(response.body)))
        .expect("Kiln validated the HTTP response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_transport;
    use http_body_util::BodyExt;
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
            let event = event_receiver.recv().await.expect("request event arrives");
            let EngineEvent::Request { id, request, body } = event else {
                panic!("expected request event")
            };
            assert_eq!(request.method, "GET");
            assert_eq!(request.target, "/health");
            assert!(body.is_empty());
            response_sender
                .send(ResponseCommand {
                    id,
                    response: HttpResponse {
                        status: 200,
                        reason: None,
                        headers: vec![("content-type".into(), "text/plain".into())],
                        body: b"kiln".to_vec(),
                    },
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
            let EngineEvent::Request { id, .. } =
                event_receiver.recv().await.expect("request event arrives")
            else {
                panic!("expected request event")
            };
            response_sender
                .send(ResponseCommand {
                    id,
                    response: HttpResponse {
                        status: 200,
                        reason: None,
                        headers: vec![("connection".into(), "close".into())],
                        body: b"kiln".to_vec(),
                    },
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
            assert!(response.ends_with("\r\n\r\nkiln"));
            assert!(!response.to_ascii_lowercase().contains("\r\ndate:"));
        });
    }
}

//! Hyper-backed HTTP protocol engine behind Kiln's runtime-private seam.
//!
//! This module deliberately contains every Hyper type used by the runtime. Node adapters consume
//! `node_http` events and owned bytes; HostOps consumes portable resource operations. Keeping this
//! module private prevents either external seam from inheriting Hyper's API or release cadence.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::node_http::{HttpRequest, HttpResponse, HttpVersion};

/// Serve one HTTP/1 connection on a Tokio-compatible byte transport.
///
/// The handler receives Hyper's request only inside this private module and returns owned response
/// bytes. `auto_date_header(false)` is load-bearing: a protocol library must not smuggle the host
/// clock around ADR 0031's realm capability boundary.
pub(crate) async fn serve_http1<I, F>(io: I, handler: F) -> Result<(), hyper::Error>
where
    I: AsyncRead + AsyncWrite + Unpin,
    F: Fn(Request<Incoming>) -> Response<Full<Bytes>> + Clone,
{
    let service = service_fn(move |request| {
        let response = handler.clone()(request);
        async move { Ok::<_, Infallible>(response) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.auto_date_header(false);
    builder.serve_connection(TokioIo::new(io), service).await
}

pub(crate) type ConnectionFuture =
    Pin<Box<dyn Future<Output = Result<(), hyper::Error>> + 'static>>;

pub(crate) fn http1_connection<I, F>(io: I, handler: F) -> ConnectionFuture
where
    I: AsyncRead + AsyncWrite + Unpin + 'static,
    F: Fn(HttpRequest) -> HttpResponse + Clone + 'static,
{
    Box::pin(serve_http1(io, move |request| {
        let version = match request.version() {
            hyper::Version::HTTP_10 => HttpVersion::Http10,
            _ => HttpVersion::Http11,
        };
        let keep_alive = !request
            .headers()
            .get(hyper::header::CONNECTION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("close"));
        let request = HttpRequest {
            method: request.method().as_str().to_owned(),
            target: request.uri().to_string(),
            version,
            headers: request
                .headers()
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
        };
        let response = handler.clone()(request);
        let mut builder = Response::builder().status(response.status);
        for (name, value) in response.headers {
            builder = builder.header(name, value);
        }
        builder
            .body(Full::new(Bytes::from(response.body)))
            .expect("Kiln validated the HTTP response")
    }))
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
            let server = tokio::task::spawn_local(serve_http1(server_io, |request| {
                assert_eq!(request.method(), hyper::Method::GET);
                assert_eq!(request.uri().path(), "/health");
                Response::builder()
                    .status(200)
                    .header("content-type", "text/plain")
                    .body(Full::new(Bytes::from_static(b"kiln")))
                    .expect("response is valid")
            }));

            let (mut sender, connection) = http1::handshake(TokioIo::new(client_io))
                .await
                .expect("client handshake succeeds");
            let client = tokio::task::spawn_local(async move {
                connection.await.expect("client connection completes")
            });
            let response = sender
                .send_request(
                    Request::builder()
                        .uri("/health")
                        .body(Full::<Bytes>::default())
                        .expect("request is valid"),
                )
                .await
                .expect("request succeeds");
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
            let server = tokio::task::spawn_local(serve_http1(io, |_| {
                Response::builder()
                    .header("connection", "close")
                    .body(Full::new(Bytes::from_static(b"kiln")))
                    .expect("response is valid")
            }));

            realm.feed_read(b"GET / HTTP/1.1\r\nhost: kiln\r\n\r\n".to_vec());
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

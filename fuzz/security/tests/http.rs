use kiln_runtime::node_http::{Http1Decoder, HttpParseError, HttpRequest, HttpRequestEvent};

fn iterations() -> usize {
    std::env::var("KILN_FUZZ_ITERATIONS")
        .map(|n| n.parse().expect("positive iteration count"))
        .unwrap_or(10_000)
}

fn decode(bytes: &[u8], width: usize, events: bool) -> Result<Vec<HttpRequest>, HttpParseError> {
    let mut decoder = Http1Decoder::default();
    let mut requests = Vec::new();
    let mut current: Option<HttpRequest> = None;
    for chunk in bytes.chunks(width.max(1)) {
        if !events {
            requests.extend(decoder.push(chunk)?);
            continue;
        }
        for event in decoder.push_events(chunk)? {
            match event {
                HttpRequestEvent::Head(head) => {
                    assert!(current.replace(head).is_none(), "head before previous end");
                }
                HttpRequestEvent::Data(data) => {
                    current
                        .as_mut()
                        .expect("body without head")
                        .body
                        .extend(data);
                }
                HttpRequestEvent::End => requests.push(current.take().expect("end without head")),
            }
        }
    }
    Ok(requests)
}

fn check(bytes: &[u8], width: usize) {
    let whole = decode(bytes, bytes.len().max(1), false);
    assert_eq!(
        whole,
        decode(bytes, width, false),
        "framing depends on transport boundaries"
    );
    assert_eq!(
        whole,
        decode(bytes, width, true),
        "streaming and buffered framing disagree"
    );
}

#[test]
fn http_framing_is_independent_of_transport_chunks() {
    bolero::check!()
        .with_iterations(iterations())
        .for_each(|bytes| {
            let bytes = &bytes[..bytes.len().min(16_384)];
            let width = bytes.first().map_or(1, |b| usize::from(*b) + 1);
            check(bytes, width);
            // Keep valid requests, binary bodies, pipelining and chunked bodies in the search space.
            let mut length = format!(
                "POST / HTTP/1.1\r\nHost: kiln\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            )
            .into_bytes();
            length.extend_from_slice(bytes);
            length.extend_from_slice(b"GET /next HTTP/1.1\r\nHost: kiln\r\n\r\n");
            check(&length, width);
            let mut chunked = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            if !bytes.is_empty() {
                chunked.extend_from_slice(format!("{:x}\r\n", bytes.len()).as_bytes());
                chunked.extend_from_slice(bytes);
                chunked.extend_from_slice(b"\r\n");
            }
            chunked.extend_from_slice(b"0\r\n\r\nGET /next HTTP/1.1\r\n\r\n");
            check(&chunked, width);
            // Mutate syntax around a real header; pure random bytes seldom reach body framing.
            let mut malformed = b"POST / HTTP/1.1\r\nContent-Length: ".to_vec();
            malformed.extend_from_slice(bytes);
            malformed.extend_from_slice(b"\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n");
            check(&malformed, width);
        });
}

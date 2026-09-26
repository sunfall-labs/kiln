//! Live-socket tests for the io_uring transport. Every test skips (passing, with a note on
//! stderr) when the kernel refuses a ring, so a seccomp'd CI container does not fail them.

use super::*;
use std::cell::Cell;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn local() -> (tokio::runtime::Runtime, tokio::task::LocalSet) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    (runtime, tokio::task::LocalSet::new())
}

fn ring_or_skip(test: &str) -> Option<Rc<Ring>> {
    match Ring::new() {
        Ok(ring) => Some(ring),
        Err(error) => {
            eprintln!("{test}: skipped, io_uring unavailable: {error}");
            None
        }
    }
}

async fn listener() -> (tokio::net::TcpListener, SocketAddr) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    (listener, address)
}

fn onto_ring(ring: &Rc<Ring>, stream: tokio::net::TcpStream) -> UringStream {
    stream.set_nodelay(true).expect("nodelay");
    let stream = stream.into_std().expect("into_std");
    ring.register(OwnedFd::from(stream))
        .expect("ring accepts the socket")
}

async fn pair(ring: &Rc<Ring>) -> (UringStream, tokio::net::TcpStream) {
    let (listener, address) = listener().await;
    let client = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect");
    let (server, _) = listener.accept().await.expect("accept");
    (onto_ring(ring, server), client)
}

async fn settle(ring: &Rc<Ring>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while ring.inner.borrow().inflight() > 0 {
        assert!(Instant::now() < deadline, "ring operations did not settle");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[test]
fn echo_round_trips_and_eof_reads_zero() {
    let Some(ring) = ring_or_skip("echo") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (mut server, mut client) = pair(&ring).await;
        for round in 0..50_u32 {
            let message = format!("ping {round}");
            client.write_all(message.as_bytes()).await.unwrap();
            let mut buffer = vec![0; message.len()];
            server.read_exact(&mut buffer).await.unwrap();
            assert_eq!(buffer, message.as_bytes());
            server.write_all(&buffer).await.unwrap();
            let mut back = vec![0; message.len()];
            client.read_exact(&mut back).await.unwrap();
            assert_eq!(back, message.as_bytes());
        }
        client.shutdown().await.unwrap();
        let mut rest = Vec::new();
        assert_eq!(server.read_to_end(&mut rest).await.unwrap(), 0);
        // Half-closed: the server can still answer, and its FIN follows the bytes.
        server.write_all(b"after eof").await.unwrap();
        server.shutdown().await.unwrap();
        let mut tail = Vec::new();
        client.read_to_end(&mut tail).await.unwrap();
        assert_eq!(tail, b"after eof");
        drop(server);
        settle(&ring).await;
    });
    let stats = ring.stats();
    assert!(stats.enters > 0 && stats.cqes > 0, "{stats:?}");
    ring.shutdown();
    assert!(!ring.inner.borrow().leak);
}

/// A peer on another thread, so every request arrives while the realm thread is parked: the
/// wake path the DEFER_TASKRUN + eventfd design got wrong (6-8 ms per request, see the module
/// documentation).
#[test]
fn round_trips_from_another_thread_are_not_delayed() {
    let Some(ring) = ring_or_skip("round trips") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (listener, address) = listener().await;
        const ROUNDS: usize = 300;
        let peer = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.set_nodelay(true).unwrap();
            let started = Instant::now();
            for _ in 0..ROUNDS {
                stream.write_all(b"ping").unwrap();
                let mut pong = [0_u8; 4];
                stream.read_exact(&mut pong).unwrap();
                assert_eq!(&pong, b"pong");
            }
            started.elapsed()
        });
        let (server, _) = listener.accept().await.unwrap();
        let mut server = onto_ring(&ring, server);
        for _ in 0..ROUNDS {
            let mut ping = [0_u8; 4];
            server.read_exact(&mut ping).await.unwrap();
            server.write_all(b"pong").await.unwrap();
        }
        let elapsed = tokio::task::spawn_blocking(move || peer.join().unwrap())
            .await
            .unwrap();
        assert!(
            elapsed < Duration::from_millis(ROUNDS as u64),
            "{ROUNDS} round trips took {elapsed:?}"
        );
        drop(server);
        settle(&ring).await;
    });
    ring.shutdown();
}

#[test]
fn large_write_to_a_slow_reader_applies_backpressure_and_arrives_intact() {
    let Some(ring) = ring_or_skip("slow reader") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (mut server, mut client) = pair(&ring).await;
        const TOTAL: usize = 24 * 1024 * 1024;
        let writer = tokio::task::spawn_local(async move {
            let chunk: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
            let mut written = 0;
            let mut pending_seen = false;
            while written < TOTAL {
                // Observe backpressure directly: a write that does not complete at once.
                let write = server.write(&chunk);
                tokio::pin!(write);
                let polled = std::future::poll_fn(|cx| match write.as_mut().poll(cx) {
                    Poll::Pending => Poll::Ready(None),
                    Poll::Ready(result) => Poll::Ready(Some(result)),
                })
                .await;
                let n = match polled {
                    Some(result) => result.unwrap(),
                    None => {
                        pending_seen = true;
                        write.await.unwrap()
                    }
                };
                written += n;
            }
            server.shutdown().await.unwrap();
            (server, pending_seen)
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut received = Vec::with_capacity(TOTAL);
        client.read_to_end(&mut received).await.unwrap();
        assert_eq!(received.len(), TOTAL);
        for (offset, block) in received.chunks(64 * 1024).enumerate() {
            assert!(
                block.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8),
                "corrupt block {offset}"
            );
        }
        let (server, pending_seen) = writer.await.unwrap();
        assert!(
            pending_seen,
            "a 24 MiB write to a stalled reader never saw backpressure"
        );
        drop(server);
        settle(&ring).await;
    });
    ring.shutdown();
}

#[test]
fn large_upload_to_a_paused_reader_stops_receiving_then_resumes() {
    let Some(ring) = ring_or_skip("paused reader") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (mut server, mut client) = pair(&ring).await;
        const TOTAL: usize = 8 * 1024 * 1024;
        let upload = tokio::task::spawn_local(async move {
            let data = vec![7_u8; TOTAL];
            client.write_all(&data).await.unwrap();
            client.shutdown().await.unwrap();
            client
        });
        // One poll submits the receive; then nobody reads for a while, and the ring must stop
        // at its high-water mark.
        std::future::poll_fn(|cx| {
            let mut none = [0_u8; 0];
            let _ = Pin::new(&mut server).poll_read(cx, &mut ReadBuf::new(&mut none));
            Poll::Ready(())
        })
        .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let before = ring.stats();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let after = ring.stats();
        // Parked, not spinning: the paused connection costs no enters while it waits.
        assert!(after.enters - before.enters < 50, "{before:?} -> {after:?}");
        {
            let inner = ring.inner.borrow();
            let conn = inner.conns[server.index as usize].as_ref().unwrap();
            assert!(conn.buffered() <= RX_HIGH + BUF_SIZE * usize::from(BUF_COUNT));
            assert_ne!(
                conn.recv,
                RecvState::Armed,
                "receive still armed above the mark"
            );
        }
        let mut received = Vec::new();
        server.read_to_end(&mut received).await.unwrap();
        assert_eq!(received.len(), TOTAL);
        assert!(received.iter().all(|b| *b == 7));
        drop(upload.await.unwrap());
        drop(server);
        settle(&ring).await;
    });
    ring.shutdown();
}

#[test]
fn dropping_mid_receive_cancels_and_closes_the_socket() {
    let Some(ring) = ring_or_skip("drop mid receive") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (mut server, mut client) = pair(&ring).await;
        // Arm and submit the receive, then drop while it waits.
        let mut buffer = [0_u8; 16];
        let read = server.read(&mut buffer);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), read)
                .await
                .is_err()
        );
        drop(server);
        settle(&ring).await;
        assert!(ring.inner.borrow().conns.iter().all(Option::is_none));
        let mut rest = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest))
            .await
            .expect("peer saw the close");
        assert!(read.is_ok() && rest.is_empty());
    });
    ring.shutdown();
}

#[test]
fn dropping_with_queued_bytes_still_delivers_them_before_the_close() {
    let Some(ring) = ring_or_skip("drop with queued bytes") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (mut server, mut client) = pair(&ring).await;
        let body = vec![b'z'; 200 * 1024];
        server.write_all(&body).await.unwrap();
        // No flush point has run: the bytes are only queued in the ring.
        drop(server);
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut received))
            .await
            .expect("close arrived")
            .expect("clean close");
        assert_eq!(received, body);
        settle(&ring).await;
    });
    ring.shutdown();
}

#[test]
fn unread_input_at_close_resets_the_connection_like_epoll() {
    let Some(ring) = ring_or_skip("reset on unread") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (server, mut client) = pair(&ring).await;
        let mut server = server;
        client.write_all(b"never read").await.unwrap();
        // Poll once so the receive is submitted and the bytes land in the ring.
        let mut one = [0_u8; 1];
        server.read_exact(&mut one).await.unwrap();
        drop(server);
        settle(&ring).await;
        let mut rest = Vec::new();
        let result = client.read_to_end(&mut rest).await;
        assert!(
            matches!(&result, Err(error) if error.kind() == io::ErrorKind::ConnectionReset),
            "{result:?}"
        );
    });
    ring.shutdown();
}

#[test]
fn many_connections_share_the_buffer_pool() {
    let Some(ring) = ring_or_skip("many connections") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (listener, address) = listener().await;
        let mut clients = Vec::new();
        let mut servers = Vec::new();
        // More connections than provided buffers, all sending at once.
        for _ in 0..usize::from(BUF_COUNT) + 64 {
            let client = tokio::net::TcpStream::connect(address).await.unwrap();
            let (server, _) = listener.accept().await.unwrap();
            clients.push(client);
            servers.push(onto_ring(&ring, server));
        }
        let payload = vec![3_u8; 3 * BUF_SIZE + 17];
        for client in &mut clients {
            client.write_all(&payload).await.unwrap();
        }
        let readers = servers.into_iter().map(|mut server| {
            let expected = payload.len();
            tokio::task::spawn_local(async move {
                let mut buffer = vec![0_u8; expected];
                server.read_exact(&mut buffer).await.unwrap();
                assert!(buffer.iter().all(|b| *b == 3));
                server.write_all(b"ok").await.unwrap();
                server
            })
        });
        let mut servers = Vec::new();
        for reader in readers.collect::<Vec<_>>() {
            servers.push(reader.await.unwrap());
        }
        for client in &mut clients {
            let mut ok = [0_u8; 2];
            client.read_exact(&mut ok).await.unwrap();
            assert_eq!(&ok, b"ok");
        }
        drop(servers);
        settle(&ring).await;
    });
    ring.shutdown();
}

#[test]
fn shutdown_with_operations_in_flight_drains_without_leaking() {
    let Some(ring) = ring_or_skip("shutdown in flight") else {
        return;
    };
    let (runtime, local) = local();
    let client = local.block_on(&runtime, async {
        let (mut server, client) = pair(&ring).await;
        // A receive armed and a send stuck behind a peer that never reads.
        let big = vec![1_u8; 32 * 1024 * 1024];
        let mut written = 0;
        while written < big.len() {
            match tokio::time::timeout(Duration::from_millis(50), server.write(&big[written..]))
                .await
            {
                Ok(result) => written += result.unwrap(),
                Err(_) => break,
            }
        }
        let mut one = [0_u8; 1];
        let _ = tokio::time::timeout(Duration::from_millis(10), server.read(&mut one)).await;
        assert!(ring.inner.borrow().inflight() > 0);
        std::mem::forget(server);
        client
    });
    let started = Instant::now();
    ring.shutdown();
    assert!(started.elapsed() < Duration::from_secs(3));
    let inner = ring.inner.borrow();
    assert_eq!(inner.inflight(), 0);
    assert!(!inner.leak);
    assert!(inner.closed);
    drop(inner);
    drop(client);
}

#[test]
fn a_closed_ring_refuses_new_sockets_and_fails_old_ones() {
    let Some(ring) = ring_or_skip("closed ring") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (mut server, _client) = pair(&ring).await;
        ring.shutdown();
        let mut buffer = [0_u8; 4];
        assert!(server.read(&mut buffer).await.is_err());
        assert!(server.write(b"x").await.is_err());
        let (listener, address) = listener().await;
        let _peer = tokio::net::TcpStream::connect(address).await.unwrap();
        let (other, _) = listener.accept().await.unwrap();
        let other = other.into_std().unwrap();
        assert!(ring.register(OwnedFd::from(other)).is_err());
    });
}

fn ring_listener(ring: &Rc<Ring>) -> (RingListener, SocketAddr) {
    ring_listener_on(ring, "127.0.0.1:0")
}

fn ring_listener_on(ring: &Rc<Ring>, bind: &str) -> (RingListener, SocketAddr) {
    let listener = std::net::TcpListener::bind(bind).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let listener = ring
        .listen(OwnedFd::from(listener))
        .expect("ring takes the listener");
    assert_eq!(listener.local_addr().unwrap(), address);
    (listener, address)
}

#[test]
fn multishot_accept_serves_a_burst_of_connections() {
    let Some(ring) = ring_or_skip("multishot accept") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (listener, address) = ring_listener(&ring);
        const CONNECTIONS: usize = ACCEPT_HIGH + 100;
        let peer = std::thread::spawn(move || {
            (0..CONNECTIONS)
                .map(|_| std::net::TcpStream::connect(address).unwrap())
                .collect::<Vec<_>>()
        });
        let mut accepted = Vec::new();
        for _ in 0..CONNECTIONS {
            let fd = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("accept stalled")
                .expect("accept succeeds");
            let stream = std::net::TcpStream::from(fd);
            assert_eq!(stream.local_addr().unwrap(), address);
            accepted.push(stream);
        }
        let clients = tokio::task::spawn_blocking(move || peer.join().unwrap())
            .await
            .unwrap();
        assert_eq!(clients.len(), CONNECTIONS);
        // Past the high-water mark the ring paused and re-armed; every connection still arrived.
        assert!(ring.inner.borrow().listeners[listener.index as usize].is_some());
        drop(listener);
        settle(&ring).await;
        assert!(ring.inner.borrow().listeners.iter().all(Option::is_none));
    });
    ring.shutdown();
    assert!(!ring.inner.borrow().leak);
}

#[test]
fn closing_a_ring_listener_stops_accepting_and_fails_pending_accepts() {
    let Some(ring) = ring_or_skip("listener close") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        // An address of its own, so no other test's listener can take the freed port and answer
        // the connect below: every other test binds 127.0.0.1.
        let (listener, address) = ring_listener_on(&ring, "127.77.0.1:0");
        let pending = tokio::task::spawn_local(listener.accept());
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            ring.inner.borrow().listeners[listener.index as usize]
                .as_ref()
                .unwrap()
                .accept,
            RecvState::Armed
        );
        drop(listener);
        let result = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .expect("pending accept woke")
            .unwrap();
        assert!(result.is_err());
        settle(&ring).await;
        assert!(ring.inner.borrow().listeners.iter().all(Option::is_none));
        // The socket is really closed: the armed multishot accept no longer holds it open. Poll
        // rather than connect once: another test in this binary may fork a child (they run
        // `ps`) while the listener is open, and the child holds a copy of every descriptor until
        // its exec closes the CLOEXEC ones -- a moment, where a leaked accept would be forever.
        let deadline = Instant::now() + Duration::from_secs(2);
        while std::net::TcpStream::connect(address).is_ok() {
            assert!(
                Instant::now() < deadline,
                "listener still accepting after close"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    ring.shutdown();
}

#[test]
fn connections_accepted_for_nobody_are_reset_when_the_listener_closes() {
    let Some(ring) = ring_or_skip("listener backlog reset") else {
        return;
    };
    let (runtime, local) = local();
    local.block_on(&runtime, async {
        let (listener, address) = ring_listener(&ring);
        // Arm, take one, and let two more land in the ring's queue unclaimed.
        let first = accept_first(&listener, address).await;
        let mut waiting: Vec<_> = (0..2)
            .map(|_| std::net::TcpStream::connect(address).unwrap())
            .collect();
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(listener);
        settle(&ring).await;
        for stream in &mut waiting {
            use std::io::Read;
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut byte = [0_u8; 1];
            let result = stream.read(&mut byte);
            assert!(
                matches!(&result, Err(error) if error.kind() == io::ErrorKind::ConnectionReset),
                "{result:?}"
            );
        }
        drop(first);
    });
    ring.shutdown();
}

/// Connect once and accept it through the ring, arming the multishot accept.
async fn accept_first(
    listener: &RingListener,
    address: SocketAddr,
) -> (std::net::TcpStream, OwnedFd) {
    let client = std::net::TcpStream::connect(address).unwrap();
    let fd = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("accept stalled")
        .unwrap();
    (client, fd)
}

#[test]
fn preference_parsing_and_forced_epoll() {
    let mut host = HostRing::new();
    host.set_preference(IoPreference::Epoll);
    assert!(host.ring().is_none());
    let mut host = HostRing::new();
    host.set_preference(IoPreference::Uring);
    // Either a ring, or a clean fallback on a kernel that refuses one.
    let _ = host.ring();
    host.shutdown();
}

/// The HTTP engine over both transports: the scenarios of the compio experiment
/// (`.scratch/compio-http`), plus a burst of pipelined requests and a response big enough to
/// exhaust the kernel's send buffer.
#[cfg(feature = "http")]
mod http {
    use super::*;
    use crate::HttpCloseMode;
    use crate::http_engine::{HttpDispatch, HttpEvent, HttpEventKind, HttpServerEngine};
    use kiln_host_abi::ResourceId;
    use std::cell::RefCell;
    use std::rc::Weak;

    const HUGE: usize = 16 * 1024 * 1024;

    #[derive(Default)]
    struct Counts {
        requests: Cell<u64>,
        closed: Cell<u64>,
        errors: RefCell<Vec<String>>,
    }

    struct Probe {
        engine: Rc<RefCell<HttpServerEngine>>,
        counts: Rc<Counts>,
        resources: RefCell<Vec<ResourceId>>,
        next_id: Cell<u64>,
    }

    impl Probe {
        fn new() -> Self {
            Self {
                engine: Rc::new(RefCell::new(HttpServerEngine::with_http3(false))),
                counts: Rc::new(Counts::default()),
                resources: RefCell::new(Vec::new()),
                next_id: Cell::new(1),
            }
        }

        fn attach(&self, transport: crate::HostTransport) {
            let id = ResourceId::new(self.next_id.get());
            self.next_id.set(self.next_id.get() + 1);
            let dispatch = Rc::new(Dispatch {
                engine: Rc::downgrade(&self.engine),
                counts: Rc::clone(&self.counts),
                resource: id,
                body: RefCell::new(Vec::new()),
                target: RefCell::new(Vec::new()),
            });
            assert!(
                self.engine
                    .borrow_mut()
                    .attach(id, transport, true, dispatch)
            );
            self.resources.borrow_mut().push(id);
        }

        async fn drain(&self) {
            for id in self.resources.borrow().iter().copied() {
                self.engine.borrow_mut().close(id, HttpCloseMode::Graceful);
            }
            tokio::time::timeout(Duration::from_secs(10), async {
                while self.engine.borrow().open_connections() != 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("HTTP connections drained");
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.engine.borrow_mut().shutdown();
        }
    }

    struct Dispatch {
        engine: Weak<RefCell<HttpServerEngine>>,
        counts: Rc<Counts>,
        resource: ResourceId,
        body: RefCell<Vec<u8>>,
        target: RefCell<Vec<u8>>,
    }

    impl HttpDispatch for Dispatch {
        fn event(&self, event: &HttpEvent<'_>) -> i32 {
            let engine = self.engine.upgrade().expect("probe alive");
            match event.kind {
                HttpEventKind::RequestHead => {
                    self.counts.requests.set(self.counts.requests.get() + 1);
                    self.body.borrow_mut().clear();
                    *self.target.borrow_mut() = event.target.to_vec();
                    if event.target == b"/abort" {
                        engine
                            .borrow_mut()
                            .close(self.resource, HttpCloseMode::Abort);
                    }
                }
                HttpEventKind::RequestData => self.body.borrow_mut().extend_from_slice(event.data),
                HttpEventKind::RequestEnd => {
                    let target = self.target.borrow().clone();
                    if target == b"/abort" {
                        return 0;
                    }
                    let body = match target.as_slice() {
                        b"/echo" => self.body.borrow().clone(),
                        b"/large" => vec![b'x'; 64 * 1024],
                        b"/huge" => vec![b'h'; HUGE],
                        _ => b"kiln\n".to_vec(),
                    };
                    let (resource, request) = (self.resource, event.id);
                    if target == b"/close" || target == b"/slow" {
                        tokio::task::spawn_local(async move {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            let mut engine = engine.borrow_mut();
                            assert!(engine.start_response(resource, request, 200, b"", 1024));
                            assert!(engine.end_response(resource, request, &body).is_some());
                            if target == b"/close" {
                                assert!(engine.close(resource, HttpCloseMode::Graceful));
                            }
                        });
                    } else {
                        let mut engine = engine.borrow_mut();
                        assert!(engine.start_response(resource, request, 200, b"", 1024));
                        assert!(engine.end_response(resource, request, &body).is_some());
                    }
                }
                HttpEventKind::Closed => self.counts.closed.set(self.counts.closed.get() + 1),
                HttpEventKind::Error => self
                    .counts
                    .errors
                    .borrow_mut()
                    .push(String::from_utf8_lossy(event.message).into_owned()),
                _ => {}
            }
            0
        }

        fn checkpoint(&self) -> i32 {
            0
        }
    }

    const SCENARIOS: &[&str] = &[
        "pipeline",
        "pipeline-burst",
        "half-close",
        "echo",
        "slow-reader",
        "huge-slow-reader",
        "abort",
    ];

    fn expected_requests(scenario: &str) -> u64 {
        match scenario {
            "pipeline" => 2,
            "pipeline-burst" => 64,
            _ => 1,
        }
    }

    async fn exercise(mut stream: tokio::net::TcpStream, scenario: &str) {
        let burst;
        let request: &[u8] = match scenario {
            "pipeline" => b"GET / HTTP/1.1\r\nHost: probe\r\n\r\nGET / HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n",
            "pipeline-burst" => {
                let mut all = b"GET / HTTP/1.1\r\nHost: probe\r\n\r\n".repeat(63);
                all.extend_from_slice(b"GET / HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n");
                burst = all;
                &burst
            }
            "half-close" => b"GET /close HTTP/1.1\r\nHost: probe\r\n\r\n",
            "echo" => b"POST /echo HTTP/1.1\r\nHost: probe\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
            "slow-reader" => b"GET /large HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n",
            "huge-slow-reader" => b"GET /huge HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n",
            "abort" => b"GET /abort HTTP/1.1\r\nHost: probe\r\n\r\n",
            other => panic!("unknown scenario {other}"),
        };
        tokio::time::timeout(Duration::from_secs(20), async {
            if scenario == "pipeline-burst" {
                stream.write_all(request).await.unwrap();
            } else {
                // Fragment the head so the server must handle incomplete input.
                for chunk in request.chunks(7) {
                    stream.write_all(chunk).await.unwrap();
                    tokio::task::yield_now().await;
                }
            }
            if scenario == "half-close" {
                stream.shutdown().await.unwrap();
            }
            if scenario.ends_with("slow-reader") {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let mut response = Vec::new();
            match stream.read_to_end(&mut response).await {
                Ok(_) => {}
                Err(error)
                    if scenario == "abort" && error.kind() == io::ErrorKind::ConnectionReset => {}
                Err(error) => panic!("{scenario}: {error}"),
            }
            if scenario == "abort" {
                assert!(response.is_empty(), "abort wrote {} bytes", response.len());
                return;
            }
            let mut offset = 0;
            for _ in 0..expected_requests(scenario) {
                let remaining = &response[offset..];
                let head_end = remaining
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap_or_else(|| panic!("{scenario}: incomplete response head"));
                let head = std::str::from_utf8(&remaining[..head_end]).unwrap();
                assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .map(|value| value.trim().parse().unwrap())
                    .expect("content-length");
                let start = offset + head_end + 4;
                let body = &response[start..start + length];
                match scenario {
                    "echo" => assert_eq!(body, b"abcde"),
                    "slow-reader" => {
                        assert!(body.len() == 64 * 1024 && body.iter().all(|b| *b == b'x'))
                    }
                    "huge-slow-reader" => {
                        assert!(body.len() == HUGE && body.iter().all(|b| *b == b'h'))
                    }
                    _ => assert_eq!(body, b"kiln\n"),
                }
                offset = start + length;
            }
            assert_eq!(offset, response.len(), "{scenario}: trailing bytes");
        })
        .await
        .unwrap_or_else(|_| panic!("{scenario}: stalled"));
    }

    fn run_scenarios(ring: Option<Rc<Ring>>, rounds: usize) {
        let (runtime, local) = local();
        local.block_on(&runtime, async {
            let probe = Probe::new();
            let (listener, address) = listener().await;
            let client = tokio::task::spawn_local(async move {
                for _ in 0..rounds {
                    for scenario in SCENARIOS {
                        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
                        stream.set_nodelay(true).unwrap();
                        exercise(stream, scenario).await;
                    }
                }
            });
            tokio::pin!(client);
            loop {
                tokio::select! {
                    result = &mut client => { result.unwrap(); break; }
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let transport = match &ring {
                            Some(ring) => crate::HostTransport(Box::new(onto_ring(ring, stream))),
                            None => {
                                stream.set_nodelay(true).unwrap();
                                crate::HostTransport(Box::new(stream))
                            }
                        };
                        probe.attach(transport);
                    }
                }
            }
            probe.drain().await;
            assert!(
                probe.counts.errors.borrow().is_empty(),
                "{:?}",
                probe.counts.errors.borrow()
            );
            let per_round: u64 = SCENARIOS.iter().map(|s| expected_requests(s)).sum();
            assert_eq!(probe.counts.requests.get(), per_round * rounds as u64);
            assert_eq!(probe.counts.closed.get(), (SCENARIOS.len() * rounds) as u64);
            if let Some(ring) = &ring {
                settle(ring).await;
                assert!(ring.inner.borrow().conns.iter().all(Option::is_none));
            }
        });
        if let Some(ring) = ring {
            ring.shutdown();
            assert!(!ring.inner.borrow().leak);
        }
    }

    #[test]
    fn http_scenarios_over_epoll() {
        run_scenarios(None, 3);
    }

    #[test]
    fn http_scenarios_over_io_uring() {
        let Some(ring) = ring_or_skip("http scenarios") else {
            return;
        };
        run_scenarios(Some(ring), 3);
    }
}

#[test]
fn kernel_release_parses_major_and_minor() {
    assert_eq!(parse_kernel_release("7.2.6-1-cachyos"), Some((7, 2)));
    assert_eq!(parse_kernel_release("6.1.0-18-amd64"), Some((6, 1)));
    assert_eq!(parse_kernel_release("6.12"), Some((6, 12)));
    assert_eq!(parse_kernel_release("5.15.0-105-generic"), Some((5, 15)));
    assert_eq!(parse_kernel_release("6"), None);
    assert_eq!(parse_kernel_release("generic"), None);
    assert_eq!(parse_kernel_release(""), None);
}

#[test]
fn auto_floor_compares_versions_not_strings() {
    // 6.10 is newer than 6.9; a string comparison would say otherwise.
    assert!((6, 10) >= AUTO_MIN_KERNEL);
    assert!((6, 1) >= AUTO_MIN_KERNEL);
    assert!((5, 19) < AUTO_MIN_KERNEL);
    assert!((6, 0) < AUTO_MIN_KERNEL);
    assert_eq!(
        auto_selects_uring(),
        running_kernel().is_some_and(|kernel| kernel >= AUTO_MIN_KERNEL)
    );
}

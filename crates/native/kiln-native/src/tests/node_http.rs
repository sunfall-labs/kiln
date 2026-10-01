use super::*;

#[test]
fn node_net_is_typescript_builtin_and_supports_exports() {
    use kiln_module_resolver::NodeBuiltin;
    assert!(native_builtin_supports_export(NodeBuiltin::Net, "connect"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Net,
        "createConnection"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Net,
        "createServer"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Net, "default"));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "resolve"));
}

#[test]
fn node_stream_is_typescript_builtin_and_supports_exports() {
    use kiln_module_resolver::NodeBuiltin;
    assert!(native_builtin_supports_export(
        NodeBuiltin::Stream,
        "Readable"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Stream,
        "Writable"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Stream,
        "addAbortSignal"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Stream,
        "finished"
    ));
}

#[test]
fn node_http_is_typescript_builtin_and_supports_exports() {
    use kiln_module_resolver::NodeBuiltin;
    assert!(native_builtin_supports_export(
        NodeBuiltin::Http,
        "IncomingMessage"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Http, "METHODS"));
    assert!(native_builtin_supports_export(NodeBuiltin::Http, "Server"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Http,
        "ServerResponse"
    ));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Http,
        "createServer"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Http, "default"));
    assert!(native_builtin_supports_export(NodeBuiltin::Http, "get"));
    assert!(native_builtin_supports_export(NodeBuiltin::Http, "request"));
}

#[test]
fn native_node_http_exports_distinct_server_and_create_server_names() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-callable-shape",
        r#"import http from "node:http";
if (typeof http.Server !== "function") throw new Error("Server is not callable");
if (typeof http.createServer !== "function") throw new Error("createServer is not callable");
if (http.Server === http.createServer) throw new Error("HTTP callables share identity");
if (http.Server.name !== "Server") throw new Error("wrong Server name");
if (http.createServer.name !== "createServer") throw new Error("wrong createServer name");
console.log("pass");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "pass\n");
}

#[test]
fn native_node_http_server_accepts_options_overload_without_listener() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-options-overload",
        r#"import http from "node:http";
const optionOnly = new http.Server({ allowHalfOpen: true });
const nullOptions = new http.Server(null);
const withListener = new http.Server({}, () => {});
const factoryOptionOnly = http.createServer({});
const factoryWithListener = http.createServer({}, () => {});
for (const server of [optionOnly, nullOptions, withListener, factoryOptionOnly, factoryWithListener]) {
  if (typeof server.close !== "function") throw new Error("missing close");
  server.close();
}
console.log("pass");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "pass\n");
}

#[test]
fn native_node_http_server_exposes_timeout_policy_used_by_frameworks() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-server-timeouts",
        r#"import http from "node:http";
const server = http.createServer(() => {});
console.log(server.timeout, server.keepAliveTimeout, server.requestTimeout);
server.setTimeout(12.9);
console.log(server.timeout);
console.log("pass");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "0 5000 300000\n12\npass\n");
}

#[test]
fn native_node_http_response_exposes_node_lifecycle_state() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-response-state",
        r#"import http from "node:http";
const server = http.createServer((request, response) => {
  console.log(response.statusCode, response.headersSent, response.writableEnded, response.destroyed);
  response.statusCode = 201;
  response.writeHead(response.statusCode, { "content-type": "text/plain" });
  console.log(response.statusCode, response.headersSent, response.writableEnded);
  response.end("ok");
  console.log(response.headersSent, response.writableEnded);
  server.close();
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    if (!request.socket || request.connection !== request.socket) throw new Error("client request socket");
    if (response.socket !== request.socket || response.connection !== request.socket) throw new Error("client response socket");
    if (socketEvents !== 1) throw new Error("client socket event count");
    response.on("data", () => {});
    response.on("end", () => {});
  });
  let socketEvents = 0;
  request.on("error", (error) => console.log("client request error event", error.code));
  request.on("socket", (socket) => {
    socketEvents++;
    if (request.socket !== socket || request.connection !== socket) throw new Error("client socket event identity");
    console.log("client socket event", request.socket === socket);
    console.log("client write after end", request.write("late", (error) => {
      console.log("client write callback", error.code);
    }));
  });
  if (request.socket !== null || request.connection !== null) throw new Error("client socket before connect");
  request.end();
});
"#,
    ) else {
        return;
    };
    assert_eq!(
        output,
        "client socket event true\nclient write after end false\nclient write callback ERR_STREAM_WRITE_AFTER_END\nclient request error event ERR_STREAM_WRITE_AFTER_END\n200 false false false\n201 true false\ntrue true\n"
    );
}

#[test]
fn native_node_http_response_headers_are_case_insensitive_and_snapshot() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-response-headers",
        r#"import http from "node:http";
const server = http.createServer((_request, response) => {
  response.setHeader("Content-Type", "text/plain");
  console.log(response.getHeader("content-type"), response.hasHeader("CONTENT-TYPE"));
  const headers = response.getHeaders();
  headers["content-type"] = "changed";
  console.log(response.getHeader("content-type"));
  response.removeHeader("CONTENT-TYPE");
  console.log(response.hasHeader("content-type"), response.getHeader("content-type"));
  response.end("ok");
  server.close();
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
    ) else {
        return;
    };
    assert_eq!(output, "text/plain true\ntext/plain\nfalse undefined\n");
}

#[test]
fn native_node_http_incoming_message_exposes_request_lifecycle_state() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-request-state",
        r#"import http from "node:http";
const server = http.createServer((request, response) => {
  console.log(request.httpVersion, request.httpVersionMajor, request.httpVersionMinor);
  console.log(request.aborted, request.complete, request.destroyed);
  request.on("end", () => {
    console.log(request.complete);
    response.end("ok");
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
    ) else {
        return;
    };
    assert_eq!(output, "1.1 1 1\nfalse false false\ntrue\n");
}

#[test]
fn native_node_http_server_rejects_invalid_options_with_a_node_code() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:http option validation: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http-invalid-options");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import http from "node:http";
for (const option of ["foo", 42, true, []]) {
  let thrown;
  try { new http.Server(option); } catch (error) { thrown = error; }
  if (!(thrown instanceof TypeError)) throw new Error("missing TypeError");
  if (thrown.code !== "ERR_INVALID_ARG_TYPE") throw new Error("missing code");
}
console.log("pass");
"#,
    )
    .expect("write invalid-options fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native invalid-options graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native invalid-options fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "pass\n");
}

#[test]
fn native_node_net_source_graph_builds_through_the_production_frontend() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:net integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-net-source");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createServer } from "node:net";
const server = createServer((socket) => {
  socket.on("data", (chunk) => {
    console.log(chunk);
    socket.end();
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
    )
    .expect("write node:net fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    assert!(graph.modules().any(|node| matches!(
        node.source,
        ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Net)
    )));
    let builder = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain);
    let report = builder
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native node:net graph builds");
    assert!(report.output.exists());

    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("start native node:net server");
    let stdout = child.stdout.take().expect("capture server stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read ephemeral listening port");
    let port: u16 = listening
        .trim()
        .parse()
        .expect("server prints numeric port");
    assert_ne!(port, 0);
    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect loopback client");
    client
        .write_all(b"native loopback")
        .expect("write client bytes");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("finish client write");
    let mut ignored = Vec::new();
    client
        .read_to_end(&mut ignored)
        .expect("server closes client");
    let status = child.wait().expect("server exits after close");
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(status.success(), "server failed: {stderr}");
    assert_eq!(rest, "native loopback\n");
}

#[test]
fn native_node_net_client_sends_final_bytes_and_reads_after_half_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:net client integration: no C compiler found");
        return;
    };
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
    let port = listener.local_addr().expect("read loopback address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept native client");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read through client EOF");
        assert_eq!(request, "final bytes");
        stream
            .write_all(b"reply")
            .expect("write reply after half-close");
    });
    let project = TestDirectory::new("kiln-native-node-net-client");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            r#"import {{ connect }} from "node:net";
const socket = connect({port}, "127.0.0.1", () => {{
  if (socket.readyState !== "open") throw new Error("socket was not open");
  socket.end("final bytes");
  if (socket.readyState !== "readOnly") throw new Error("socket was not read-only");
  console.log("write after end", socket.write("late", (error) => console.log("write callback", error.code)));
}});
let reply = "";
socket.setEncoding("utf8");
socket.on("data", chunk => reply += chunk);
socket.on("error", error => console.log("write error event", error.code));
socket.on("end", () => {{}});
socket.on("close", () => {{
  if (socket.readyState !== "closed") throw new Error("socket was not closed");
  console.log(reply);
}});
"#
        ),
    )
    .expect("write node:net client fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("client"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native node:net client builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:net client");
    server.join().expect("loopback server completes");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"write after end false\nwrite callback ERR_STREAM_WRITE_AFTER_END\nwrite error event ERR_STREAM_WRITE_AFTER_END\nreply\n"
    );
}

#[test]
fn native_node_http_server_responds_after_node_net_client_half_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP half-close integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-half-close");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        include_str!(
            "../../../../../conformance/node/upstream-adapted/http-server-allow-half-open.mjs"
        ),
    )
    .expect("write HTTP half-close fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native HTTP half-close fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native HTTP half-close fixture");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll native fixture") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stuck native fixture");
            break child.wait().expect("reap stuck native fixture");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("capture stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(
        status.success(),
        "status={status} stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(stdout, "pass\n");
}

#[test]
fn native_node_http_server_handles_two_pipelined_requests_on_one_socket() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP pipelining integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-pipelining");
    let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
    let port = reservation.local_addr().expect("read reserved port").port();
    drop(reservation);
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            r#"import {{ createServer }} from "node:http";
let requests = 0;
const server = createServer((request, response) => {{
  console.log(request.url);
  response.end(request.url);
  requests += 1;
  if (requests === 2) server.close();
}});
server.listen({port}, "127.0.0.1");
"#
        ),
    )
    .expect("write HTTP pipelining fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native HTTP pipelining fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native HTTP pipelining fixture");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut client = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(client) => break client,
            Err(error) if Instant::now() < deadline => {
                if let Some(status) = child.try_wait().expect("poll native server") {
                    let mut stderr = String::new();
                    child
                        .stderr
                        .take()
                        .expect("capture stderr")
                        .read_to_string(&mut stderr)
                        .expect("read stderr");
                    panic!("server exited before listen: {status}; stderr={stderr:?}");
                }
                let _ = error;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                child.kill().expect("kill server stuck before listen");
                child.wait().expect("reap server stuck before listen");
                panic!("server did not listen before deadline: {error}");
            }
        }
    };
    client
        .set_nonblocking(true)
        .expect("make HTTP client nonblocking");
    client
            .write_all(
                b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\nGET /second HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .expect("write pipelined requests");
    let mut response = String::new();
    let mut chunk = [0_u8; 4096];
    let response_deadline = Instant::now() + Duration::from_secs(10);
    while !response.contains("/second") {
        match client.read(&mut chunk) {
            Ok(0) => break,
            Ok(length) => {
                response.push_str(&String::from_utf8_lossy(&chunk[..length]));
                if response.len() > 64 * 1024 {
                    child.kill().expect("kill server repeating response bytes");
                    child.wait().expect("reap server repeating response bytes");
                    let prefix = &response[..response.floor_char_boundary(512)];
                    panic!(
                        "HTTP bridge repeated response bytes; length={} prefix={prefix:?}",
                        response.len()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= response_deadline {
                    child.kill().expect("kill server stuck during responses");
                    child.wait().expect("reap server stuck during responses");
                    panic!("timed out reading pipelined responses; partial={response:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                panic!("read both pipelined responses: {error}; partial response={response:?}")
            }
        }
    }
    let first = response.find("/first").unwrap_or_else(|| {
        let status = child
            .try_wait()
            .expect("poll server missing first response")
            .unwrap_or_else(|| {
                child.kill().expect("kill server missing first response");
                child.wait().expect("reap server missing first response")
            });
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!("first response body missing: {response:?}; status={status}; stderr={stderr:?}")
    });
    let second = response.find("/second").unwrap_or_else(|| {
        child.kill().expect("kill server missing second response");
        child.wait().expect("reap server missing second response");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!("second response body missing: {response:?}; stderr={stderr:?}")
    });
    assert!(
        first < second,
        "responses preserve request order: {response:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll native server exit") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill server stuck after responses");
            break child.wait().expect("reap server stuck after responses");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = String::new();
    child
        .stdout
        .take()
        .expect("capture stdout")
        .read_to_string(&mut rest)
        .expect("read stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(
        status.success(),
        "status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert_eq!(rest, "/first\n/second\n");
}

#[test]
fn native_node_http_source_graph_serves_one_live_tokio_request() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:http integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http-source");
    let entry = project.path.join("main.mjs");
    fs::write(
            &entry,
            r#"import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log(request.method, request.url, request.httpVersion);
  if (!request.socket || request.connection !== request.socket) throw new Error("request socket");
  if (response.req !== request) throw new Error("response request");
  if (response.socket !== request.socket || response.connection !== request.socket) throw new Error("response socket");
  let body = "";
  request.on("data", chunk => body += chunk);
  request.on("end", () => {
    console.log(body);
    response.setHeader("Content-Type", "text/plain");
    response.writeHead(201);
    response.write("echo:");
    response.end(body);
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
        )
        .expect("write node:http fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    assert!(graph.modules().any(|node| matches!(
        node.source,
        ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Http)
    )));
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native node:http graph builds");

    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("start native node:http server");
    let stdout = child.stdout.take().expect("capture server stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read ephemeral listening port");
    let port: u16 = listening
        .trim()
        .parse()
        .expect("server prints numeric port");
    assert_ne!(port, 0);

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set HTTP client timeout");
    client
            .write_all(
                b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc",
            )
            .expect("write first HTTP request fragment");
    client
        .write_all(b"123")
        .expect("write second HTTP request fragment");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("finish HTTP request");
    let mut response = String::new();
    let read_result = client.read_to_string(&mut response);
    if let Err(error) = read_result {
        let status = child.wait().expect("server exits after failed HTTP read");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!(
            "HTTP read failed: {error}; status={status}; response={response:?}; stderr={stderr:?}"
        );
    }
    if !response.starts_with("HTTP/1.1 201 Created\r\n") {
        child.kill().expect("kill server after unexpected response");
        let status = child.wait().expect("reap server after unexpected response");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!("status={status} response={response:?} stderr={stderr:?}");
    }
    let response_lower = response.to_ascii_lowercase();
    assert!(
        response_lower.contains("content-type: text/plain\r\n"),
        "{response:?}"
    );
    assert!(
        response_lower.contains("connection: close\r\n"),
        "{response:?}"
    );
    assert!(
        response.ends_with("\r\n5\r\necho:\r\n6\r\nabc123\r\n0\r\n\r\n"),
        "{response:?}"
    );

    let status = child
        .wait()
        .expect("server exits after listener and socket close");
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read request callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(status.success(), "server failed: {stderr}");
    assert_eq!(rest, "POST /echo 1.1\nabc123\n");
}

#[test]
fn native_node_http_client_response_set_encoding_delivers_text_chunks() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native setEncoding integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-client-set-encoding");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import http from "node:http";
const server = http.createServer((request, response) => {
  response.writeHead(200, { "content-type": "text/plain" });
  response.end("kiln");
});
server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  http.get({ host: "127.0.0.1", port, path: "/" }, (response) => {
    response.setEncoding("utf8");
    let body = "";
    let kinds = "";
    response.on("data", chunk => { kinds += typeof chunk; body += chunk; });
    response.on("end", () => {
      console.log(response.statusCode, response.headers["content-type"], kinds, body);
      server.close();
    });
  });
});
"#,
    )
    .expect("write node:http setEncoding fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("client"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native node:http setEncoding graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:http setEncoding program");
    assert!(
        output.status.success(),
        "status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Recorded from node v25: `200 text/plain string kiln`.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "200 text/plain string kiln\n"
    );
}

#[test]
fn native_node_http_server_close_waits_for_delayed_response_after_half_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP delayed-close integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-delayed-close");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createServer } from "node:http";
let socket_closes = 0;
const server = createServer((request, response) => {
  console.log("request");
  request.socket.on("close", () => {
    socket_closes += 1;
    console.log(`socket-close:${socket_closes}`);
  });
  setTimeout(() => { console.log("timer"); response.end("delayed"); }, 0);
  console.log("before-close");
  server.close(() => console.log("server-close"));
  console.log("after-close", request.socket.readyState);
});
server.httpAllowHalfOpen = true;
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
    )
    .expect("write delayed-close fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native delayed-close fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native delayed-close fixture");
    let stdout = child.stdout.take().expect("capture server stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read delayed-close listening port");
    let port: u16 = listening
        .trim()
        .parse()
        .expect("server prints numeric port");
    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set HTTP client timeout");
    client
        .write_all(b"GET /delayed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write HTTP request");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close HTTP request");
    let mut response = String::new();
    if let Err(error) = client.read_to_string(&mut response) {
        let _ = child.kill();
        let status = child
            .wait()
            .expect("reap delayed-close server after read failure");
        panic!("read delayed HTTP response: {error}; status={status}; response={response:?}");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll delayed-close server exit") {
            break status;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .expect("kill delayed-close server after timeout");
            break child
                .wait()
                .expect("reap delayed-close server after timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read delayed-close callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture server stderr")
        .read_to_string(&mut stderr)
        .expect("read server stderr");
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
    );
    // The body arrives whole with `end`, so it is length-declared rather than chunked. What
    // this test pins is the ordering - the response completes before the close - not the
    // framing, so both facts are asserted separately.
    assert!(
        response.contains("\r\ncontent-length: 7\r\n"),
        "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert!(
        response.ends_with("\r\n\r\ndelayed"),
        "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert!(status.success(), "server failed: {stderr}");
    assert_eq!(rest.matches("socket-close:").count(), 1, "stdout={rest:?}");
    assert_eq!(
        rest,
        "request\nbefore-close\nafter-close open\ntimer\nsocket-close:1\nserver-close\n"
    );
}

#[test]
fn native_node_http_socket_destroy_uses_abort_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP destroy integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-destroy");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log("destroy");
  request.socket.destroy();
  server.close(() => console.log("closed"));
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
    )
    .expect("write HTTP destroy fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native HTTP destroy fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native HTTP destroy fixture");
    let stdout = child.stdout.take().expect("capture destroy stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read destroy listening port");
    let port: u16 = listening.trim().parse().expect("numeric destroy port");
    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
    client
        .write_all(b"GET /destroy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write destroy request");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll destroy server") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill destroy server after timeout");
            break child.wait().expect("reap destroy server after timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read destroy callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture destroy stderr")
        .read_to_string(&mut stderr)
        .expect("read destroy stderr");
    assert!(
        status.success(),
        "status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert_eq!(rest, "destroy\nclosed\n");
}

#[test]
fn native_node_http2_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http2-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import http2, { createServer, createSecureServer } from 'node:http2';\n",
            "let threw = false;\n",
            "try {\n",
            "  createServer();\n",
            "} catch (e) {\n",
            "  threw = true;\n",
            "}\n",
            "console.log('http2-ts:', typeof http2, typeof createServer, typeof createSecureServer, threw);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("http2_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run http2 binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("http2-ts: object function function true"));
}

#[test]
fn native_node_http2_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http2-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const http2 = require('node:http2');\n",
            "let threw = false;\n",
            "try {\n",
            "  http2.createSecureServer();\n",
            "} catch (e) {\n",
            "  threw = true;\n",
            "}\n",
            "console.log('cjs-http2:', typeof http2, typeof http2.createServer, threw);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("http2_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run http2 cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-http2: object function true"));
}

#[test]
fn native_node_stream_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-stream-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import Stream, { Readable, Writable, addAbortSignal, finished } from 'node:stream';\n",
            "const r = new Readable();\n",
            "r.push('native stream data');\n",
            "const data = r.read();\n",
            "console.log('stream-ts:', typeof Stream, typeof Readable, typeof Writable, data.toString() === 'native stream data');\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("stream_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run stream binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("stream-ts: function function function true"));
}

#[test]
fn native_node_stream_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-stream-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const Stream = require('node:stream');\n",
            "console.log('cjs-stream:', typeof Stream, typeof Stream.Readable, typeof Stream.Writable);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("stream_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run stream cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-stream: function function function"));
}

#[test]
fn native_node_https_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-https-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import https, { createServer } from 'node:https';\n",
            "let threw = false;\n",
            "try {\n",
            "  createServer();\n",
            "} catch (e) {\n",
            "  threw = e.message.includes('requires a TLS host capability');\n",
            "}\n",
            "console.log('https-ts:', typeof https, typeof createServer, threw);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("https_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run https binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("https-ts: object function true"));
}

#[test]
fn native_node_https_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-https-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const https = require('node:https');\n",
            "let threw = false;\n",
            "try {\n",
            "  https.createServer();\n",
            "} catch (e) {\n",
            "  threw = e.message.includes('requires a TLS host capability');\n",
            "}\n",
            "console.log('cjs-https:', typeof https, typeof https.createServer, threw);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("https_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run https cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-https: object function true"));
}

#[test]
fn native_node_net_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-net-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import net, { connect, createConnection, createServer } from 'node:net';\n",
            "console.log('net-ts:', typeof net, typeof connect, typeof createConnection, typeof createServer, net.default === net);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("net_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run net binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("net-ts: object function function function true"));
}

#[test]
fn native_node_net_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-net-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const net = require('node:net');\n",
            "console.log('cjs-net:', typeof net, typeof net.connect, typeof net.createConnection, typeof net.createServer, net.default === net);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("net_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run net cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-net: object function function function true"));
}

#[test]
fn native_node_http_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import http, { createServer, Server, ServerResponse, METHODS } from 'node:http';\n",
            "console.log('http-ts:', typeof http, typeof createServer, typeof Server, typeof ServerResponse, Array.isArray(METHODS), http.default === http);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("http_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run http binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("http-ts: object function function function true true"));
}

#[test]
fn native_node_http_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const http = require('node:http');\n",
            "console.log('cjs-http:', typeof http, typeof http.createServer, typeof http.Server, typeof http.ServerResponse, Array.isArray(http.METHODS), http.default === http);\n",
        ),
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("http_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run http cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("cjs-http: object function function function true true"));
}

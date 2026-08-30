//! End-to-end tests for the daemon, driven over a real unix socket.
//!
//! They run against [`StubEngine`] rather than the production database: the contract the server
//! is written against is [`QueryEngine`], and a stub is the only way to provoke the failure modes
//! that matter here - notably a query that panics, which a correct engine never does.

use crate::dispatch::{Next, dispatch};
use crate::framing::MAX_MESSAGE_BYTES;
use crate::{Client, Daemon, DaemonError, ShutdownHandle};
use kiln_db::{FileId, Interface, Parsed, Project, QueryEngine, Revision, Stats};
use kiln_proto::{NativeProfile, PROTOCOL_VERSION, Request, Response, SourceHash};
use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

/// A source text that makes [`StubEngine::parse`] panic, so the server's panic isolation can be
/// tested from the outside rather than by reaching into it.
const POISON: &str = "__panic_while_parsing__";

/// A minimal, non-memoizing [`QueryEngine`]. It is not trying to be incremental - that is the
/// database's job and its own tests' concern. It only has to be a faithful enough engine that
/// the server's dispatch, locking and error paths are exercised.
#[derive(Default)]
struct StubEngine {
    paths: Vec<String>,
    interned: HashMap<String, FileId>,
    sources: HashMap<u32, String>,
    revision: u64,
    stats: Stats,
}

impl StubEngine {
    fn text(&self, file: FileId) -> &str {
        self.sources.get(&file.0).map(String::as_str).unwrap_or("")
    }
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h: u64, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn exports_of(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("export "))
        .map(str::to_string)
        .collect()
}

impl QueryEngine for StubEngine {
    fn intern(&mut self, path: &str) -> FileId {
        if let Some(file) = self.interned.get(path) {
            return *file;
        }
        let file = FileId(self.paths.len() as u32);
        self.paths.push(path.to_string());
        self.interned.insert(path.to_string(), file);
        file
    }

    fn set_source(&mut self, file: FileId, text: String) {
        self.sources.insert(file.0, text);
        self.revision += 1;
        self.stats.revision = self.revision;
    }

    fn source(&self, file: FileId) -> Option<&str> {
        self.sources.get(&file.0).map(String::as_str)
    }

    fn source_files(&self) -> Vec<(FileId, String)> {
        let mut files = self
            .sources
            .keys()
            .filter_map(|id| {
                self.paths
                    .get(*id as usize)
                    .map(|path| (FileId(*id), path.clone()))
            })
            .collect::<Vec<_>>();
        files.sort_by_key(|(file, _)| *file);
        files
    }

    fn remove_source(&mut self, file: FileId) {
        if self.sources.remove(&file.0).is_some() {
            self.revision += 1;
            self.stats.revision = self.revision;
        }
    }

    fn parse(&mut self, file: FileId) -> Parsed {
        let text = self.text(file);
        assert_ne!(
            text, POISON,
            "stub engine asked to parse the poison fixture"
        );
        let parsed = Parsed {
            implementation_hash: hash(text.as_bytes()),
            exports: exports_of(text),
            lines: text.lines().count(),
            module: frontend_oxc::parse("stub.js", text),
        };
        self.stats.parse_computed += 1;
        parsed
    }

    fn ir(&mut self, file: FileId) -> kiln_db::Compiled {
        let parsed = self.parse(file);
        self.stats.ir_computed += 1;
        kiln_ir::compile(&parsed.module)
    }

    fn interface(&mut self, file: FileId) -> Interface {
        let exports = exports_of(self.text(file));
        self.stats.interface_computed += 1;
        Interface {
            interface_hash: hash(exports.join("\n").as_bytes()),
            exports,
        }
    }

    fn project(&mut self) -> Project {
        let files: Vec<FileId> = self.sources.keys().copied().map(FileId).collect();
        let mut exports = 0;
        let mut rolled = 0u64;
        for file in &files {
            let interface = self.interface(*file);
            exports += interface.exports.len();
            rolled ^= interface.interface_hash;
        }
        self.stats.project_computed += 1;
        Project {
            modules: files.len(),
            exports,
            interface_hash: rolled,
        }
    }

    fn revision(&self) -> Revision {
        Revision(self.revision)
    }

    fn stats(&self) -> Stats {
        self.stats
    }

    fn reset_stats(&mut self) {
        self.stats = Stats {
            revision: self.revision,
            ..Stats::default()
        };
    }
}

/// A scratch directory plus, usually, a daemon serving inside it. Cleans both up on drop so a
/// failing assertion cannot leave a socket or a temp directory behind.
struct Fixture {
    dir: PathBuf,
    socket: PathBuf,
    stop: Option<ShutdownHandle>,
    serving: Option<JoinHandle<()>>,
}

impl Fixture {
    /// A scratch directory with no daemon in it, for the bind-time tests.
    fn empty() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kiln-d-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create scratch directory");
        let socket = dir.join("daemon.sock");
        Self {
            dir,
            socket,
            stop: None,
            serving: None,
        }
    }

    fn started() -> Self {
        let mut fixture = Self::empty();
        fixture.start();
        fixture
    }

    fn start(&mut self) {
        let daemon = Daemon::bind_with(&self.socket, StubEngine::default()).expect("bind");
        self.stop = Some(daemon.shutdown_handle());
        self.serving = Some(std::thread::spawn(move || daemon.serve()));
    }

    fn client(&self) -> Client {
        Client::connect(&self.socket).expect("connect to the daemon")
    }

    /// Stop the daemon and wait for the accept loop, failing the test if it does not return.
    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.shutdown();
        }
        if let Some(serving) = self.serving.take() {
            serving.join().expect("the accept loop must not panic");
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run `body` on another thread and fail rather than hang if it does not finish. Every test that
/// could deadlock goes through this: a hung test suite tells you nothing about what broke.
fn within<T: Send + 'static>(limit: Duration, body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(body());
    });
    rx.recv_timeout(limit).expect("timed out")
}

#[test]
fn request_and_response_round_trip_over_a_socket() {
    let fixture = Fixture::started();
    let mut client = fixture.client();

    let pid = client.handshake().expect("handshake");
    assert_eq!(pid, std::process::id(), "the daemon reports its own pid");

    let set = client
        .request(&Request::SetSource {
            path: "a.js".into(),
            text: "export a\nexport b\nconst hidden = 1\n".into(),
        })
        .expect("set source");
    assert!(matches!(set, Response::Ok), "got {set:?}");

    let Response::Checked(summary) = client
        .request(&Request::Check {
            path: "a.js".into(),
        })
        .expect("check")
    else {
        panic!("expected a module summary");
    };
    assert_eq!(summary.path, "a.js");
    assert_eq!(summary.exports, vec!["a".to_string(), "b".to_string()]);
    assert_ne!(summary.implementation_hash, summary.interface_hash);

    let Response::Project(project) = client.request(&Request::Project).expect("project") else {
        panic!("expected a project summary");
    };
    assert_eq!(project.modules, 1);
    assert_eq!(project.exports, 2);

    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(stats.revision, 1, "one SetSource, one revision");
    assert_eq!(stats.parse_computed, 1);

    assert!(matches!(
        client.request(&Request::ResetStats).expect("reset"),
        Response::Ok
    ));
    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(stats.parse_computed, 0, "reset clears counters");
    assert_eq!(stats.revision, 1, "but not the revision");
}

#[test]
fn sync_reconciles_omissions_and_rejects_duplicates_without_partial_removal() {
    let mut engine = StubEngine::default();
    let alpha = "export alpha\n";
    let beta = "export beta\n";
    dispatch(
        &mut engine,
        Request::SetSource {
            path: "alpha.js".into(),
            text: alpha.into(),
        },
    );
    dispatch(
        &mut engine,
        Request::SetSource {
            path: "beta.js".into(),
            text: beta.into(),
        },
    );

    let (response, next) = dispatch(
        &mut engine,
        Request::Sync {
            files: vec![
                SourceHash {
                    path: "alpha.js".into(),
                    hash: kiln_db::content_hash(alpha),
                },
                SourceHash {
                    path: "alpha.js".into(),
                    hash: kiln_db::content_hash(alpha),
                },
            ],
        },
    );
    assert_eq!(next, Next::Continue);
    assert!(
        matches!(response, Response::Error { ref message } if message.contains("duplicate paths") && message.contains("alpha.js")),
        "got {response:?}"
    );
    assert_eq!(
        engine.source_files().len(),
        2,
        "rejection is all-or-nothing"
    );

    let (response, _) = dispatch(
        &mut engine,
        Request::Sync {
            files: vec![SourceHash {
                path: "alpha.js".into(),
                hash: kiln_db::content_hash(alpha),
            }],
        },
    );
    assert!(matches!(response, Response::Needed { paths } if paths.is_empty()));
    assert_eq!(
        engine.source_files(),
        vec![(FileId(0), "alpha.js".to_owned())],
        "omitted held files are removed before Needed is returned"
    );

    let (response, _) = dispatch(
        &mut engine,
        Request::Sync {
            files: vec![SourceHash {
                path: "gamma.js".into(),
                hash: 7,
            }],
        },
    );
    assert!(matches!(response, Response::Needed { paths } if paths == ["gamma.js"]));
}

#[test]
fn native_run_round_trips_through_the_daemon_and_reuses_artifacts() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    let source_path = fixture.dir.join("native.js");
    let path = source_path.to_string_lossy().into_owned();
    let project_root = fixture.dir.to_string_lossy().into_owned();
    client
        .request(&Request::SetSource {
            path: path.clone(),
            text: "console.log('native daemon', 6 * 7);".to_owned(),
        })
        .expect("upload native source");

    let request = Request::NativeRun {
        path,
        project_root,
        profile: NativeProfile::Dev,
    };
    let first = client.request(&request).expect("first native request");
    let Response::NativeRan(first) = first else {
        if let Response::Error { ref message } = first
            && message.contains("no supported C compiler")
        {
            eprintln!("skipping native daemon integration: {message}");
            return;
        }
        panic!("expected native run, got {first:?}");
    };
    assert_eq!(first.exit_code, Some(0));
    assert_eq!(first.stdout, "native daemon 42\n");
    assert!(!first.build.app_object_cache_hit);

    let Response::NativeRan(second) = client.request(&request).expect("cached native request")
    else {
        panic!("expected cached native run");
    };
    assert_eq!(second.stdout, first.stdout);
    assert!(second.build.generated_c_cache_hit);
    assert!(second.build.app_object_cache_hit);
    assert!(second.build.runtime_object_cache_hit);
    assert!(second.build.launcher_object_cache_hit);
    assert!(second.build.executable_cache_hit);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn production_daemon_native_shared_build_returns_summary_and_cache_hits() {
    let fixture = Fixture::empty();
    let source = fixture.dir.join("native-shared.js");
    std::fs::write(&source, "console.log('shared daemon', 6 * 7);\n").expect("write shared source");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let path = std::fs::canonicalize(&source)
        .expect("canonical source")
        .to_string_lossy()
        .into_owned();
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let request = Request::NativeSharedBuild {
        path: path.clone(),
        project_root: root.clone(),
        output: None,
        profile: NativeProfile::Dev,
    };

    let first = client.request(&request).expect("first shared build");
    let Response::NativeSharedBuilt(first) = first else {
        if let Response::Error { ref message } = first
            && message.contains("no supported C compiler")
        {
            eprintln!("skipping native shared daemon integration: {message}");
            stop.shutdown();
            within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
            return;
        }
        panic!("expected shared build, got {first:?}");
    };
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let expected_output = Path::new(&root)
        .join(".kiln/lib")
        .join("native-shared")
        .with_extension(extension);
    assert_eq!(first.output_path, expected_output.to_string_lossy());
    let expected_header = expected_output.with_extension("h");
    assert_eq!(first.header_path, expected_header.to_string_lossy());
    let header = std::fs::read_to_string(&first.header_path).expect("read shared build header");
    assert!(header.contains(&format!("int {}(", first.module_symbol)));
    assert!(!first.generated_c_cache_hit);
    assert!(!first.object_cache_hit);
    assert!(!first.library_cache_hit);
    assert!(expected_output.is_file());

    let Response::NativeSharedBuilt(second) =
        client.request(&request).expect("cached shared build")
    else {
        panic!("expected cached shared build");
    };
    assert_eq!(second.output_path, first.output_path);
    assert_eq!(second.header_path, first.header_path);
    assert!(second.generated_c_cache_hit);
    assert!(second.object_cache_hit);
    assert!(second.library_cache_hit);
    assert_eq!(second.library_bytes, first.library_bytes);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn native_shared_build_rejects_header_source_and_output_collisions() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");

    client
        .request(&Request::SetSource {
            path: "embed.h".into(),
            text: "console.log('embedded');\n".into(),
        })
        .expect("upload header-named source");
    let response = client
        .request(&Request::NativeSharedBuild {
            path: "embed.h".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
            output: Some("embed".into()),
            profile: NativeProfile::Dev,
        })
        .expect("header/source collision response");
    assert!(
        matches!(&response, Response::Error { message } if message.contains("header") && message.contains("overwrite source")),
        "got {response:?}"
    );

    client
        .request(&Request::SetSource {
            path: "embed.js".into(),
            text: "console.log('embedded');\n".into(),
        })
        .expect("upload source");
    let response = client
        .request(&Request::NativeSharedBuild {
            path: "embed.js".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
            output: Some("embed.H".into()),
            profile: NativeProfile::Dev,
        })
        .expect("header/output collision response");
    assert!(
        matches!(&response, Response::Error { message } if message.contains("header path") && message.contains("differ")),
        "got {response:?}"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn production_daemon_native_shared_graph_build_returns_summary_and_cache_hits() {
    let fixture = Fixture::empty();
    let dependency = fixture.dir.join("shared-dependency.js");
    let entry = fixture.dir.join("shared-entry.js");
    std::fs::write(&dependency, "export const answer = 42;\n").expect("write dependency");
    std::fs::write(
        &entry,
        "import { answer } from './shared-dependency.js'; console.log(answer);\n",
    )
    .expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let entry = std::fs::canonicalize(&entry).expect("canonical entry");
    let dependency = std::fs::canonicalize(&dependency).expect("canonical dependency");
    for path in [&dependency, &entry] {
        client
            .request(&Request::SetSource {
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("upload source");
    }
    let request = Request::NativeSharedBuild {
        path: entry.to_string_lossy().into_owned(),
        project_root: root.clone(),
        output: None,
        profile: NativeProfile::Dev,
    };

    let first = client.request(&request).expect("shared graph response");
    let Response::NativeSharedBuilt(first) = first else {
        if let Response::Error { ref message } = first
            && message.contains("no supported C compiler")
        {
            eprintln!("skipping native shared graph daemon integration: {message}");
            stop.shutdown();
            within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
            return;
        }
        panic!("expected shared graph build, got {first:?}");
    };
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let expected_output = Path::new(&root)
        .join(".kiln/lib")
        .join("shared-entry")
        .with_extension(extension);
    assert_eq!(first.output_path, expected_output.to_string_lossy());
    let expected_header = expected_output.with_extension("h");
    assert_eq!(first.header_path, expected_header.to_string_lossy());
    assert!(
        !first.module_symbol.is_empty(),
        "wrapper symbol is reported"
    );
    let header = std::fs::read_to_string(&first.header_path).expect("read shared graph header");
    assert!(header.contains(&format!("int {}(", first.module_symbol)));
    assert!(!first.generated_c_cache_hit);
    assert!(!first.object_cache_hit);
    assert!(!first.library_cache_hit);
    assert!(expected_output.is_file());

    let Response::NativeSharedBuilt(second) = client
        .request(&request)
        .expect("cached shared graph response")
    else {
        panic!("expected cached shared graph build");
    };
    assert_eq!(second.output_path, first.output_path);
    assert_eq!(second.header_path, first.header_path);
    assert_eq!(second.module_symbol, first.module_symbol);
    assert!(second.generated_c_cache_hit);
    assert!(second.object_cache_hit);
    assert!(second.library_cache_hit);
    assert_eq!(second.library_bytes, first.library_bytes);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn many_requests_reuse_one_connection() {
    // The point of the daemon is that the second request is cheap, which presumes the connection
    // stays open across requests rather than being one-shot.
    let fixture = Fixture::started();
    let mut client = fixture.client();
    for index in 0..64 {
        let response = client
            .request(&Request::SetSource {
                path: format!("m{index}.js"),
                text: format!("export e{index}\n"),
            })
            .expect("set source");
        assert!(matches!(response, Response::Ok));
    }
    let Response::Project(project) = client.request(&Request::Project).expect("project") else {
        panic!("expected a project summary");
    };
    assert_eq!(project.modules, 64);
}

#[test]
fn protocol_mismatch_is_rejected_and_the_connection_dropped() {
    let fixture = Fixture::started();
    let mut client = fixture.client();

    let response = client
        .request(&Request::Ping {
            protocol: PROTOCOL_VERSION + 1,
        })
        .expect("the daemon answers before hanging up");
    let Response::Error { message } = response else {
        panic!("a version-skewed client must be refused, got {response:?}");
    };
    assert!(
        message.contains(&(PROTOCOL_VERSION + 1).to_string())
            && message.contains(&PROTOCOL_VERSION.to_string()),
        "the error must name both versions: {message}"
    );

    // Refusing the request but continuing to serve the connection would let a skewed client go
    // on to issue requests we cannot interpret.
    let after = client.request(&Request::Project);
    assert!(
        after.is_err(),
        "the connection must be closed, got {after:?}"
    );

    // ...and only that connection.
    let mut fresh = fixture.client();
    assert!(fresh.handshake().is_ok(), "the daemon still serves others");
}

#[test]
fn stale_socket_from_a_crashed_daemon_is_reclaimed() {
    let mut fixture = Fixture::empty();

    // A daemon killed with SIGKILL leaves the socket file behind: std does not unlink on drop,
    // so binding and dropping reproduces exactly what a crash leaves on disk.
    let corpse = UnixListener::bind(&fixture.socket).expect("bind the doomed listener");
    drop(corpse);
    assert!(
        fixture.socket.exists(),
        "the crash must leave a path behind"
    );

    fixture.start();
    let mut client = fixture.client();
    assert!(
        client.handshake().is_ok(),
        "a crash must not wedge the project"
    );
}

#[test]
fn a_plain_file_in_the_socket_path_is_cleared() {
    let mut fixture = Fixture::empty();
    std::fs::write(&fixture.socket, b"not a socket").expect("write the obstruction");

    fixture.start();
    let mut client = fixture.client();
    assert!(client.handshake().is_ok());
}

#[test]
fn a_live_daemon_is_not_displaced() {
    // The mirror image of stale-socket recovery: reclaiming a socket someone is listening on
    // would give one project two daemons with divergent views of the same files.
    let fixture = Fixture::started();
    let error = Daemon::bind_with(&fixture.socket, StubEngine::default())
        .err()
        .expect("binding over a live daemon must fail");
    assert!(
        matches!(&error, DaemonError::AlreadyRunning { socket } if socket == &fixture.socket),
        "got {error:?}"
    );
}

#[test]
fn a_panicking_query_costs_one_request_not_the_daemon() {
    let fixture = Fixture::started();
    let mut client = fixture.client();

    client
        .request(&Request::SetSource {
            path: "boom.js".into(),
            text: POISON.into(),
        })
        .expect("set source");

    // The default panic hook prints a backtrace notice here; that is the daemon logging a bug,
    // and is expected noise in this test's output.
    let response = client
        .request(&Request::Check {
            path: "boom.js".into(),
        })
        .expect("the daemon answers rather than dying");
    let Response::Error { message } = response else {
        panic!("expected an error response, got {response:?}");
    };
    assert!(message.contains("internal error"), "{message}");

    // Same connection, then a fresh one: neither the client's session nor the daemon is lost.
    assert!(matches!(
        client.request(&Request::Project).expect("still serving"),
        Response::Project(_)
    ));
    let mut fresh = fixture.client();
    assert!(fresh.handshake().is_ok());
}

#[test]
fn a_silent_client_does_not_block_another() {
    let fixture = Fixture::started();

    // Connects, sends a partial frame, and never finishes it - the cheapest denial of service
    // there is against a server that handles connections in turn.
    let mut sulking = UnixStream::connect(&fixture.socket).expect("connect");
    std::io::Write::write_all(&mut sulking, b"{\"Ping\":{\"protocol\":1}").expect("partial write");

    let socket = fixture.socket.clone();
    let pid = within(Duration::from_secs(5), move || {
        let mut client = Client::connect(&socket).expect("connect");
        client.handshake().expect("handshake")
    });
    assert_eq!(pid, std::process::id());

    // And shutting down must not wait on the silent client either, which is what `Fixture::drop`
    // asserts by joining the accept loop.
    drop(sulking);
}

#[test]
fn a_malformed_frame_costs_one_response_not_the_connection() {
    let fixture = Fixture::started();
    let mut raw = UnixStream::connect(&fixture.socket).expect("connect");
    std::io::Write::write_all(&mut raw, b"{ this is not a request }\n").expect("write");

    let mut reader = std::io::BufReader::new(raw.try_clone().expect("clone"));
    let frame = crate::framing::read_frame(&mut reader, MAX_MESSAGE_BYTES)
        .expect("read")
        .expect("a response");
    let response: Response = serde_json::from_str(&frame).expect("a well-formed response");
    assert!(
        matches!(&response, Response::Error { message } if message.contains("malformed")),
        "got {response:?}"
    );

    // Line framing resynchronises on its own, so the connection is still usable.
    let request = serde_json::to_string(&Request::Ping {
        protocol: PROTOCOL_VERSION,
    })
    .expect("encode");
    crate::framing::write_frame(&mut raw, &request).expect("write");
    let frame = crate::framing::read_frame(&mut reader, MAX_MESSAGE_BYTES)
        .expect("read")
        .expect("a response");
    let response: Response = serde_json::from_str(&frame).expect("a well-formed response");
    assert!(
        matches!(response, Response::Pong { .. }),
        "got {response:?}"
    );
}

#[test]
fn shutdown_request_stops_the_daemon() {
    let mut fixture = Fixture::started();
    let mut client = fixture.client();

    let response = client.request(&Request::Shutdown).expect("shutdown");
    assert!(matches!(response, Response::Ok), "got {response:?}");

    // `stop` joins the accept loop; if `Shutdown` did not stop it, the handle's own signal would
    // hide the bug, so wait for the loop to end on the request alone.
    let serving = fixture.serving.take().expect("serving");
    let joined = within(Duration::from_secs(5), move || serving.join());
    joined.expect("the accept loop must not panic");
    fixture.stop = None;
}

#[test]
fn the_socket_is_unlinked_when_the_daemon_stops() {
    let mut fixture = Fixture::empty();
    fixture.start();
    assert!(fixture.socket.exists());
    fixture.stop();
    assert!(
        !fixture.socket.exists(),
        "a clean shutdown should not leave the next daemon a stale path to clear"
    );
}

#[test]
fn check_reads_from_disk_when_nothing_was_pushed() {
    let fixture = Fixture::started();
    let module = fixture.dir.join("on-disk.js");
    std::fs::write(&module, "export fromDisk\n").expect("write fixture module");

    let mut client = fixture.client();
    let Response::Checked(summary) = client
        .request(&Request::Check {
            path: module.to_string_lossy().into_owned(),
        })
        .expect("check")
    else {
        panic!("expected a module summary");
    };
    assert_eq!(summary.exports, vec!["fromDisk".to_string()]);
}

#[test]
fn a_pushed_buffer_wins_over_the_file_on_disk() {
    // The editor case: the buffer is unsaved, so re-reading the file would silently check the
    // wrong text.
    let fixture = Fixture::started();
    let module = fixture.dir.join("buffered.js");
    std::fs::write(&module, "export saved\n").expect("write fixture module");
    let path = module.to_string_lossy().into_owned();

    let mut client = fixture.client();
    client
        .request(&Request::SetSource {
            path: path.clone(),
            text: "export unsaved\n".into(),
        })
        .expect("set source");

    let Response::Checked(summary) = client.request(&Request::Check { path }).expect("check")
    else {
        panic!("expected a module summary");
    };
    assert_eq!(summary.exports, vec!["unsaved".to_string()]);
}

#[test]
fn checking_a_missing_file_is_an_error_not_a_panic() {
    let fixture = Fixture::started();
    let missing = fixture.dir.join("nope.js").to_string_lossy().into_owned();

    let mut client = fixture.client();
    let response = client
        .request(&Request::Check {
            path: missing.clone(),
        })
        .expect("check");
    assert!(
        matches!(&response, Response::Error { message } if message.contains(&missing)),
        "got {response:?}"
    );
    assert!(matches!(
        client.request(&Request::Project).expect("still serving"),
        Response::Project(_)
    ));
}

#[test]
fn socket_path_is_derived_from_the_project_root_alone() {
    let fixture = Fixture::empty();
    let direct = crate::socket_path(&fixture.dir).expect("socket path");
    let indirect = crate::socket_path(
        &fixture.dir.join("..").join(
            fixture
                .dir
                .file_name()
                .expect("the scratch directory has a name"),
        ),
    )
    .expect("socket path");
    assert_eq!(
        direct, indirect,
        "two spellings of one root must find one daemon"
    );
    assert_eq!(
        direct.file_name(),
        Some(Path::new("daemon.sock").as_os_str())
    );
}

#[test]
fn the_default_daemon_serves_the_real_query_engine() {
    // Everything above runs against `StubEngine`, so nothing else would notice if the real
    // database stopped fitting behind `Daemon::bind` - which is the entry point the CLI and the
    // timing harness actually use.
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());

    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    client
        .request(&Request::SetSource {
            path: "real.ts".into(),
            text: concat!(
                "export interface Shape { width: number }\n",
                "const decoy = 'export const fake = 0';\n",
                "export const answer: number = 1;\n",
            )
            .into(),
        })
        .expect("set source");
    let response = client
        .request(&Request::Check {
            path: "real.ts".into(),
        })
        .expect("check");
    let Response::Checked(summary) = response else {
        panic!("expected checked response, got {response:?}");
    };
    assert_eq!(summary.exports, ["answer"]);
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);

    // Prime the whole-project query, then prove a body-only edit parsed by Oxc still cuts off at
    // the interface exactly as the original query-engine contract requires.
    assert!(matches!(
        client.request(&Request::Project).expect("project"),
        Response::Project(_)
    ));
    client
        .request(&Request::ResetStats)
        .expect("reset counters");
    client
        .request(&Request::SetSource {
            path: "real.ts".into(),
            text: concat!(
                "export interface Shape { width: number }\n",
                "const decoy = 'export const fake = 0';\n",
                "export const answer: number = 2;\n",
            )
            .into(),
        })
        .expect("edit source");
    client
        .request(&Request::Project)
        .expect("project after edit");
    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(stats.parse_computed, 1);
    assert_eq!(stats.interface_computed, 1);
    assert_eq!(stats.project_computed, 0);
    assert_eq!(stats.early_cutoff, 1);

    client
        .request(&Request::SetSource {
            path: "real.ts".into(),
            text: "export const answer: = ;".into(),
        })
        .expect("set malformed source");
    let Response::Checked(summary) = client
        .request(&Request::Check {
            path: "real.ts".into(),
        })
        .expect("check malformed source")
    else {
        panic!("expected checked response");
    };
    assert!(
        !summary.diagnostics.is_empty(),
        "syntax diagnostics must cross the daemon protocol"
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_daemon_executes_cached_bytecode_and_structures_compile_failures() {
    let fixture = Fixture::empty();
    let disk_module = fixture.dir.join("production-disk.ts");
    std::fs::write(&disk_module, "export const loaded = 1;\n").expect("write disk module");
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");

    let Response::Checked(disk_summary) = client
        .request(&Request::Check {
            path: disk_module.to_string_lossy().into_owned(),
        })
        .expect("production check reads disk")
    else {
        panic!("expected checked response");
    };
    assert_eq!(disk_summary.exports, ["loaded"]);

    let path = "run.ts".to_string();
    client
        .request(&Request::SetSource {
            path: path.clone(),
            text: concat!(
                "let n: number = 0;\n",
                "while (n < 3) n += 1;\n",
                "{ let n = 9; console.log('inner', n); }\n",
                "{ let undefined = 7; console.log('shadow', undefined); }\n",
                "console.log('kiln', n);\n",
            )
            .into(),
        })
        .expect("set source");
    let Response::Ran(first) = client
        .request(&Request::Run {
            path: path.clone(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("run")
    else {
        panic!("expected run response");
    };
    assert!(first.succeeded, "{:?}", first.diagnostics);
    assert_eq!(first.stdout, "inner 9\nshadow 7\nkiln 3\n");
    assert!(first.instructions > 0);

    client
        .request(&Request::ResetStats)
        .expect("reset counters");
    let Response::Ran(second) = client
        .request(&Request::Run {
            path: path.clone(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("warm run")
    else {
        panic!("expected run response");
    };
    assert_eq!(
        second.stdout, first.stdout,
        "side effects replay on every run"
    );
    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(stats.parse_computed, 0);
    assert_eq!(stats.ir_computed, 0);
    assert_eq!(stats.validated, 1, "the bytecode query should be reused");

    client
        .request(&Request::SetSource {
            path: path.clone(),
            text: "function main() {}\nmain();\n".into(),
        })
        .expect("set unsupported source");
    let Response::Ran(failed) = client
        .request(&Request::Run {
            path,
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("compile failure is a response")
    else {
        panic!("expected run response");
    };
    assert!(!failed.succeeded);
    assert!(failed.stdout.is_empty());
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.start.is_some()
                && diagnostic.message.contains("function declaration")),
        "{:?}",
        failed.diagnostics
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_daemon_executes_an_acyclic_module_graph_and_reuses_module_ir() {
    let fixture = Fixture::empty();
    let dependency = fixture.dir.join("dependency.js");
    let entry = fixture.dir.join("entry.js");
    std::fs::write(
        &dependency,
        "console.log('dependency'); export const answer = 42;\n",
    )
    .expect("write dependency");
    std::fs::write(
        &entry,
        "import { answer } from './dependency.js'; console.log(answer);\n",
    )
    .expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    for path in [&dependency, &entry] {
        let path = std::fs::canonicalize(path).expect("canonical source");
        client
            .request(&Request::SetSource {
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("set source");
    }

    let response = client
        .request(&Request::Run {
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
        })
        .expect("run graph");
    let Response::Ran(first) = response else {
        panic!("expected run response");
    };
    assert!(first.succeeded, "{:?}", first.diagnostics);
    assert_eq!(first.stdout, "dependency\n42\n");
    assert!(first.instructions > 0);

    client.request(&Request::ResetStats).expect("reset stats");
    let Response::Ran(second) = client
        .request(&Request::Run {
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
        })
        .expect("warm graph run")
    else {
        panic!("expected run response");
    };
    assert_eq!(second.stdout, first.stdout);
    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats response");
    };
    assert_eq!(stats.parse_computed, 0);
    assert_eq!(stats.ir_computed, 0);

    std::fs::write(
        &dependency,
        "import './entry.js'; export const answer = 42;\n",
    )
    .expect("write cycle");
    client
        .request(&Request::SetSource {
            path: std::fs::canonicalize(&dependency)
                .expect("canonical dependency")
                .to_string_lossy()
                .into_owned(),
            text: "import './entry.js'; export const answer = 42;\n".into(),
        })
        .expect("set cycle");
    let Response::Ran(cycle) = client
        .request(&Request::Run {
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root,
        })
        .expect("cycle is structured")
    else {
        panic!("expected run response");
    };
    assert!(!cycle.succeeded);
    assert!(
        cycle
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cycle"))
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn production_daemon_sync_removes_omitted_dependency_before_a_warm_run() {
    let fixture = Fixture::empty();
    let dependency = fixture.dir.join("sync-dependency.js");
    let entry = fixture.dir.join("sync-entry.js");
    std::fs::write(
        &dependency,
        "console.log('stale dependency'); export const answer = 42;\n",
    )
    .expect("write dependency");
    std::fs::write(
        &entry,
        "import { answer } from './sync-dependency.js'; console.log(answer);\n",
    )
    .expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let entry = std::fs::canonicalize(entry).expect("canonical entry");
    let dependency = std::fs::canonicalize(dependency).expect("canonical dependency");
    let entry_path = entry.to_string_lossy().into_owned();
    let dependency_path = dependency.to_string_lossy().into_owned();
    let entry_source = std::fs::read_to_string(&entry).expect("read entry");
    let dependency_source = std::fs::read_to_string(&dependency).expect("read dependency");
    for (path, text) in [
        (dependency_path.clone(), dependency_source),
        (entry_path.clone(), entry_source.clone()),
    ] {
        assert!(matches!(
            client
                .request(&Request::SetSource { path, text })
                .expect("upload source"),
            Response::Ok
        ));
    }

    let project_root = fixture.dir.to_string_lossy().into_owned();
    let Response::Ran(first) = client
        .request(&Request::Run {
            path: entry_path.clone(),
            project_root: project_root.clone(),
        })
        .expect("initial run")
    else {
        panic!("expected initial run response");
    };
    assert!(first.succeeded, "{:?}", first.diagnostics);
    assert_eq!(first.stdout, "stale dependency\n42\n");

    let sync = client
        .request(&Request::Sync {
            files: vec![SourceHash {
                path: entry_path.clone(),
                hash: kiln_db::content_hash(&entry_source),
            }],
        })
        .expect("sync authoritative view");
    assert!(matches!(sync, Response::Needed { paths } if paths.is_empty()));

    let Response::Ran(second) = client
        .request(&Request::Run {
            path: entry_path,
            project_root,
        })
        .expect("warm run after removal")
    else {
        panic!("expected warm run response");
    };
    assert!(!second.succeeded, "removed dependency must not execute");
    assert!(second.stdout.is_empty(), "stale side effects must not run");
    assert!(
        second
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("source is not synchronized")),
        "missing dependency should be reported, got {:?}",
        second.diagnostics
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn relative_overlay_updates_are_reflected_by_canonical_graph_execution() {
    let fixture = Fixture::empty();
    let entry = fixture.dir.join("overlay.js");
    std::fs::write(&entry, "console.log('disk');\n").expect("write entry");
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();

    for (source, expected) in [
        ("console.log('first');\n", "first\n"),
        ("console.log('second');\n", "second\n"),
    ] {
        client
            .request(&Request::SetSource {
                path: "overlay.js".into(),
                text: source.into(),
            })
            .expect("set relative overlay");
        let Response::Ran(result) = client
            .request(&Request::Run {
                path: "overlay.js".into(),
                project_root: root.clone(),
            })
            .expect("run overlay")
        else {
            panic!("expected run response");
        };
        assert!(result.succeeded, "{:?}", result.diagnostics);
        assert_eq!(result.stdout, expected);
    }

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn production_daemon_native_graph_matches_interpreter_order_and_reuses_artifacts() {
    let fixture = Fixture::empty();
    let dependency = fixture.dir.join("native-dependency.js");
    let entry = fixture.dir.join("native-entry.js");
    std::fs::write(
        &dependency,
        "console.log('native dependency'); export const answer = 42;\n",
    )
    .expect("write dependency");
    std::fs::write(
        &entry,
        "import { answer } from './native-dependency.js'; console.log(answer);\n",
    )
    .expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let canonical_dependency = std::fs::canonicalize(&dependency).expect("canonical dependency");
    let canonical_entry = std::fs::canonicalize(&entry).expect("canonical entry");
    for path in [&canonical_dependency, &canonical_entry] {
        client
            .request(&Request::SetSource {
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("set source");
    }
    let request = Request::NativeRun {
        path: canonical_entry.to_string_lossy().into_owned(),
        project_root: root,
        profile: NativeProfile::Dev,
    };
    let first = client.request(&request).expect("native graph run");
    let Response::NativeRan(first) = first else {
        if let Response::Error { ref message } = first
            && message.contains("no supported C compiler")
        {
            eprintln!("skipping native graph integration: {message}");
            return;
        }
        panic!("expected native run, got {first:?}");
    };
    assert_eq!(first.exit_code, Some(0));
    assert_eq!(first.stdout, "native dependency\n42\n");

    let Response::NativeRan(second) = client.request(&request).expect("cached native graph run")
    else {
        panic!("expected cached native run");
    };
    assert_eq!(second.stdout, first.stdout);
    assert!(second.build.generated_c_cache_hit);
    assert!(second.build.app_object_cache_hit);

    client
        .request(&Request::SetSource {
            path: canonical_dependency.to_string_lossy().into_owned(),
            text: "import './native-entry.js'; export const answer = 42;\n".into(),
        })
        .expect("set cycle");
    let response = client.request(&request).expect("cycle response");
    let Response::Error { message } = response else {
        panic!("expected native cycle error, got {response:?}");
    };
    assert!(message.contains("cycle"), "{message}");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

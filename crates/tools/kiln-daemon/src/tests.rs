//! End-to-end tests for the daemon, driven over a real unix socket.
//!
//! They run against [`StubEngine`] rather than the production database: the contract the server
//! is written against is [`QueryEngine`], and a stub is the only way to provoke the failure modes
//! that matter here - notably a query that panics, which a correct engine never does.

use crate::dispatch::{
    execute_prepared_run, install_graph_gate, install_run_gate, install_test_gate,
    interpreted_argv, prepare_run,
};
use crate::framing::MAX_MESSAGE_BYTES;
use crate::{Client as RawClient, Daemon, DaemonError, ShutdownHandle};
use kiln_db::{FileId, Interface, Parsed, Project, QueryEngine, Revision, Stats};
use kiln_proto::{
    NativeProfile, NativeSemantics, PROTOCOL_VERSION, Request, Response, SourceHash, SourceView,
    StepBudget, SyncId, TestMode, TestOutcome,
};
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
    source_revisions: HashMap<u32, Revision>,
    revision: u64,
    stats: Stats,
    set_source_calls: usize,
    panic_on_set_source_call: Option<usize>,
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

#[test]
fn source_check_loads_project_ambient_declarations_without_runtime_edges() {
    let entry = "/project/main.ts";
    let ambient = "/project/ambient.d.ts";
    let modules = vec![
        (
            entry.to_owned(),
            kiln_frontend_oxc::parse(
                entry,
                "function add(value: Scalar) { return value + 1; } const limit: Scalar = ambientLimit;",
            ),
        ),
        (
            ambient.to_owned(),
            kiln_frontend_oxc::parse(
                ambient,
                "type Scalar = number; declare const ambientLimit: Scalar;",
            ),
        ),
    ];

    let result = crate::server::check_source_types(
        entry,
        &modules
            .into_iter()
            .map(|(p, m)| (p, std::sync::Arc::new(m)))
            .collect::<Vec<_>>(),
        &[],
    );

    assert!(result.complete, "checker result must be complete");
    assert!(
        result.diagnostics.is_empty(),
        "ambient declarations were not loaded: {:?}",
        result.diagnostics
    );
    assert!(
        result.types.iter().any(|ty| {
            ty.path == entry
                && ty.role == kiln_hir::source_types::TypeRole::Parameter
                && ty.class == kiln_hir::source_types::TypeClass::Number
                && ty.display == "number"
        }),
        "ambient Scalar alias did not resolve the parameter as number: {:?}",
        result.types
    );
}

#[test]
fn source_check_uses_nearest_uploaded_config_and_isolates_external_ambient_types() {
    use std::sync::Arc;
    let entry = "project/src/main.ts";
    let modules = vec![
        (
            entry.into(),
            Arc::new(kiln_frontend_oxc::parse(
                entry,
                "const value: number = null;",
            )),
        ),
        (
            "project/node_modules/unrelated/index.d.ts".into(),
            Arc::new(kiln_frontend_oxc::parse(
                "project/node_modules/unrelated/index.d.ts",
                "declare const broken: MissingType;",
            )),
        ),
    ];
    let outer = (
        "project/tsconfig.json".into(),
        Arc::<[u8]>::from(br#"{"compilerOptions":{"strict":true,"skipLibCheck":true}}"#.as_slice()),
    );
    let nearest = (
        "project/src/tsconfig.jsonc".into(),
        Arc::<[u8]>::from(
            br#"{"compilerOptions":{"strict":false,"skipLibCheck":true}}"#.as_slice(),
        ),
    );
    let relaxed = crate::server::check_source_types(entry, &modules, &[outer.clone(), nearest]);
    assert!(relaxed.complete, "{:?}", relaxed.diagnostics);
    assert!(relaxed.diagnostics.is_empty(), "{:?}", relaxed.diagnostics);
    let strict = crate::server::check_source_types(entry, &modules, &[outer]);
    assert!(strict.complete, "{:?}", strict.diagnostics);
    assert!(
        strict.diagnostics.iter().any(|d| d.code == Some(2322)),
        "{:?}",
        strict.diagnostics
    );
    assert!(
        !strict.diagnostics.iter().any(|d| d.code == Some(2304)),
        "{:?}",
        strict.diagnostics
    );
}

#[test]
fn source_check_limits_ambient_roots_to_entry_package_but_resolves_imported_packages() {
    use std::sync::Arc;

    let entry = "/workspace/packages/app/src/main.ts";
    let global = "/workspace/packages/app/types/global.d.ts";
    let unrelated = "/workspace/packages/other/index.d.ts";
    let imported = "/workspace/packages/app/node_modules/actual/index.d.ts";
    let modules = vec![
        (
            entry.into(),
            Arc::new(kiln_frontend_oxc::parse(
                entry,
                "import { importedValue } from 'actual'; const ambient: number = globalValue; const mismatch: number = importedValue;",
            )),
        ),
        (
            global.into(),
            Arc::new(kiln_frontend_oxc::parse(
                global,
                "declare const globalValue: number;",
            )),
        ),
        (
            unrelated.into(),
            Arc::new(kiln_frontend_oxc::parse(
                unrelated,
                "declare const leaked: MissingFromOtherPackage;",
            )),
        ),
        (
            imported.into(),
            Arc::new(kiln_frontend_oxc::parse(
                imported,
                "export declare const importedValue: string;",
            )),
        ),
    ];
    let package = |path: &str, text: &[u8]| (path.into(), Arc::<[u8]>::from(text));
    let auxiliary = vec![
        package("/workspace/package.json", br#"{"name":"workspace"}"#),
        package("/workspace/packages/app/package.json", br#"{"name":"app"}"#),
        package(
            "/workspace/packages/other/package.json",
            br#"{"name":"other"}"#,
        ),
        package(
            "/workspace/packages/app/node_modules/actual/package.json",
            br#"{"name":"actual","types":"index.d.ts"}"#,
        ),
    ];

    let result = crate::server::check_source_types(entry, &modules, &auxiliary);

    assert!(result.complete, "checker result must be complete");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == Some(2322)),
        "the imported package declaration was not resolved: {:?}",
        result.diagnostics
    );
    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == Some(2304)),
        "the sibling package leaked into ambient roots: {:?}",
        result.diagnostics
    );
}

/// Test clients keep the current protocol source view automatically. This lets the older daemon
/// tests focus on query behavior while still exercising the v16 CAS contract.
struct Client {
    raw: RawClient,
    source_view: SourceView,
}

impl Client {
    fn connect(socket: &Path) -> std::io::Result<Self> {
        Ok(Self {
            raw: RawClient::connect(socket)?,
            source_view: SourceView(0),
        })
    }

    fn handshake(&mut self) -> std::io::Result<(u32, u64)> {
        self.raw.handshake()
    }

    fn request(&mut self, request: &Request) -> std::io::Result<Response> {
        let mut response = self.raw_request(request)?;
        if matches!(response, Response::StaleSourceView { .. })
            && matches!(
                request,
                Request::SetSource { .. }
                    | Request::Check { .. }
                    | Request::Run { .. }
                    | Request::NodeRun { .. }
                    | Request::NativeBuild { .. }
                    | Request::NativeBuildAtRevision { .. }
                    | Request::NativeSharedBuild { .. }
                    | Request::NativeRun { .. }
                    | Request::Project { .. }
            )
        {
            response = self.raw_request(request)?;
        }
        Ok(match response {
            Response::SourceUpdated { .. } => Response::Ok,
            response => response,
        })
    }

    fn raw_request(&mut self, request: &Request) -> std::io::Result<Response> {
        self.request_at_view(request, self.source_view)
    }

    fn request_at_view(
        &mut self,
        request: &Request,
        source_view: SourceView,
    ) -> std::io::Result<Response> {
        let mut request = request.clone();
        match &mut request {
            Request::SetSource {
                source_view: view, ..
            }
            | Request::Check {
                source_view: view, ..
            }
            | Request::Run {
                source_view: view, ..
            }
            | Request::NativeBuild {
                source_view: view, ..
            }
            | Request::NativeBuildAtRevision {
                source_view: view, ..
            }
            | Request::NativeSharedBuild {
                source_view: view, ..
            }
            | Request::NativeRun {
                source_view: view, ..
            }
            | Request::Project { source_view: view } => *view = source_view,
            _ => {}
        }
        let response = self.raw.request(&request)?;
        match response {
            Response::SourceUpdated { source_view: view }
            | Response::Synced { source_view: view } => {
                self.source_view = view;
            }
            Response::StaleSourceView { current } => self.source_view = current,
            _ => {}
        }
        Ok(response)
    }

    fn source_view(&self) -> SourceView {
        self.source_view
    }
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
        self.set_source_calls += 1;
        if self.panic_on_set_source_call == Some(self.set_source_calls) {
            panic!("stub engine set_source panic");
        }
        if self.sources.get(&file.0) == Some(&text) {
            return;
        }
        self.sources.insert(file.0, text);
        self.revision += 1;
        self.stats.revision = self.revision;
        self.source_revisions
            .insert(file.0, Revision(self.revision));
    }

    fn source(&self, file: FileId) -> Option<&str> {
        self.sources.get(&file.0).map(String::as_str)
    }

    fn source_revision(&self, file: FileId) -> Option<Revision> {
        self.source_revisions.get(&file.0).copied()
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
            self.source_revisions.remove(&file.0);
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
            module: std::sync::Arc::new(kiln_frontend_oxc::parse("stub.js", text)),
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
        self.stats.interface_computed += 1;
        Interface::from_exports(exports_of(self.text(file)))
    }

    fn project(&mut self) -> Project {
        let files: Vec<FileId> = self.sources.keys().copied().map(FileId).collect();
        let mut interfaces = Vec::with_capacity(files.len());
        for file in &files {
            let path = self.paths[file.0 as usize].clone();
            let interface = self.interface(*file);
            interfaces.push((path, interface));
        }
        self.stats.project_computed += 1;
        Project::from_interfaces(
            interfaces
                .iter()
                .map(|(path, interface)| (path.as_str(), interface)),
        )
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

/// The map a client sends when it has resolved no packages, which is every test here: these
/// exercise the source snapshot, and a fabricated resolution would be a fact none of them
/// established.
fn empty_resolutions() -> kiln_proto::ResolutionMap {
    kiln_proto::ResolutionMap::new("/project", Vec::new()).expect("an empty map is valid")
}

fn begin_sync(client: &mut Client, files: &[(&str, &str)]) -> (SyncId, Vec<String>) {
    let response = client
        .raw_request(&Request::Sync {
            files: files
                .iter()
                .map(|(path, text)| SourceHash {
                    path: (*path).to_owned(),
                    hash: kiln_proto::source_digest(text),
                })
                .collect(),
            resolutions: empty_resolutions(),
        })
        .expect("begin sync");
    let Response::Needed { sync_id, paths } = response else {
        panic!("expected Needed response, got {response:?}");
    };
    (sync_id, paths)
}

fn stage_sync(client: &mut Client, sync_id: SyncId, path: &str, text: &str) -> Response {
    client
        .raw_request(&Request::StageSource {
            sync_id,
            path: path.to_owned(),
            text: text.to_owned(),
        })
        .expect("stage source")
}

fn commit_sync(client: &mut Client, sync_id: SyncId) -> Response {
    client
        .raw_request(&Request::CommitSync { sync_id })
        .expect("commit sync")
}

#[test]
fn staged_sync_is_invisible_until_commit_and_is_visible_to_other_clients_afterward() {
    let fixture = Fixture::started();
    let mut staging = fixture.client();
    let mut observer = fixture.client();
    staging.handshake().expect("staging handshake");
    observer.handshake().expect("observer handshake");

    let (sync_id, paths) = begin_sync(&mut staging, &[("staged.js", "export staged\n")]);
    assert_eq!(paths, ["staged.js"]);
    assert!(matches!(
        stage_sync(&mut staging, sync_id, "staged.js", "export staged\n"),
        Response::Ok
    ));
    let Response::Project(project) = observer
        .raw_request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project before commit")
    else {
        panic!("expected project response");
    };
    assert_eq!(project.modules, 0);

    let Response::Synced { source_view } = commit_sync(&mut staging, sync_id) else {
        panic!("expected synced response");
    };
    assert_eq!(source_view, SourceView(1));
    let Response::Project(project) = observer
        .request_at_view(&Request::Project { source_view }, source_view)
        .expect("project after commit")
    else {
        panic!("expected project response");
    };
    assert_eq!(project.modules, 1);
}

#[test]
fn stale_commit_is_consumed_without_mutating_the_authoritative_sources() {
    let fixture = Fixture::started();
    let mut first = fixture.client();
    let mut second = fixture.client();
    first.handshake().expect("first handshake");
    second.handshake().expect("second handshake");

    let (first_sync, _) = begin_sync(&mut first, &[("first.js", "export first\n")]);
    stage_sync(&mut first, first_sync, "first.js", "export first\n");
    let (second_sync, _) = begin_sync(&mut second, &[("second.js", "export second\n")]);
    stage_sync(&mut second, second_sync, "second.js", "export second\n");
    assert!(matches!(
        commit_sync(&mut second, second_sync),
        Response::Synced {
            source_view: SourceView(1)
        }
    ));

    assert!(matches!(
        commit_sync(&mut first, first_sync),
        Response::StaleSourceView {
            current: SourceView(1)
        }
    ));
    let Response::Project(project) = second
        .request_at_view(
            &Request::Project {
                source_view: SourceView(1),
            },
            SourceView(1),
        )
        .expect("project after stale commit")
    else {
        panic!("expected project response");
    };
    assert_eq!(project.modules, 1);
}

#[test]
fn stale_source_operations_are_rejected_after_a_competing_commit() {
    let fixture = Fixture::started();
    let mut writer = fixture.client();
    let mut stale = fixture.client();
    writer.handshake().expect("writer handshake");
    stale.handshake().expect("stale handshake");
    let (sync_id, _) = begin_sync(&mut writer, &[("source.js", "export source\n")]);
    stage_sync(&mut writer, sync_id, "source.js", "export source\n");
    assert!(matches!(
        commit_sync(&mut writer, sync_id),
        Response::Synced {
            source_view: SourceView(1)
        }
    ));
    stale.source_view = SourceView(1);
    let old_view = stale.source_view();
    let (other_sync, _) = begin_sync(&mut writer, &[("source.js", "export changed\n")]);
    stage_sync(&mut writer, other_sync, "source.js", "export changed\n");
    assert!(matches!(
        commit_sync(&mut writer, other_sync),
        Response::Synced {
            source_view: SourceView(2)
        }
    ));
    let response = stale.request_at_view(
        &Request::Check {
            source_view: old_view,
            path: "source.js".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        },
        old_view,
    );
    assert!(matches!(
        response.expect("stale check response"),
        Response::StaleSourceView {
            current: SourceView(2)
        }
    ));
    let response = stale
        .request_at_view(
            &Request::Run {
                stream_output: false,
                node: false,
                wall_clock: false,
                steps: StepBudget::Unmetered,
                source_view: old_view,
                path: "source.js".into(),
                project_root: fixture.dir.to_string_lossy().into_owned(),
                args: Vec::new(),
            },
            old_view,
        )
        .expect("stale run response");
    assert!(matches!(
        response,
        Response::StaleSourceView {
            current: SourceView(2)
        }
    ));
}

#[test]
fn warm_noop_sync_still_advances_the_source_view() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    let files = [("warm.js", "export warm\n")];
    let (first, _) = begin_sync(&mut client, &files);
    stage_sync(&mut client, first, "warm.js", files[0].1);
    assert!(matches!(
        commit_sync(&mut client, first),
        Response::Synced {
            source_view: SourceView(1)
        }
    ));
    let (second, paths) = begin_sync(&mut client, &files);
    assert!(paths.is_empty());
    assert!(matches!(
        commit_sync(&mut client, second),
        Response::Synced {
            source_view: SourceView(2)
        }
    ));
}

#[test]
fn sync_rejects_bad_digest_and_incomplete_commit_without_losing_pending_state() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    let (sync_id, _) = begin_sync(&mut client, &[("checked.js", "export checked\n")]);
    assert!(matches!(
        stage_sync(&mut client, sync_id, "checked.js", "wrong\n"),
        Response::Error { .. }
    ));
    assert!(matches!(
        commit_sync(&mut client, sync_id),
        Response::Error { .. }
    ));
    assert!(matches!(
        stage_sync(&mut client, sync_id, "checked.js", "export checked\n"),
        Response::Ok
    ));
    assert!(matches!(
        commit_sync(&mut client, sync_id),
        Response::Synced {
            source_view: SourceView(1)
        }
    ));
}

#[test]
fn commit_sync_panic_drops_the_cached_graph_snapshot() {
    let fixture = Fixture::empty();
    let source = fixture.dir.join("panic-commit.js");
    std::fs::write(&source, "export const answer = 1;\n").expect("write source");
    let source = std::fs::canonicalize(source).expect("canonical source");
    let root = std::fs::canonicalize(&fixture.dir).expect("canonical root");
    let source_path = source.to_string_lossy().into_owned();
    let daemon = Daemon::bind_with(
        &fixture.socket,
        StubEngine {
            panic_on_set_source_call: Some(2),
            ..StubEngine::default()
        },
    )
    .expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: source_path.clone(),
            text: "export const answer = 1;\n".into(),
        })
        .expect("set initial source");
    assert!(matches!(
        client
            .request(&Request::Check {
                source_view: SourceView(0),
                path: source_path.clone(),
                project_root: root.to_string_lossy().into_owned(),
            })
            .expect("prime graph cache"),
        Response::Checked(_)
    ));
    let Response::Stats(before) = client.request(&Request::Stats).expect("stats before") else {
        panic!("expected stats response");
    };
    assert_eq!(before.graph_cache_entries, 1);

    let (sync_id, needed) = begin_sync(
        &mut client,
        &[(source_path.as_str(), "export const answer = 2;\n")],
    );
    assert_eq!(needed.as_slice(), std::slice::from_ref(&source_path));
    assert!(matches!(
        stage_sync(
            &mut client,
            sync_id,
            &source_path,
            "export const answer = 2;\n"
        ),
        Response::Ok
    ));
    assert!(matches!(
        commit_sync(&mut client, sync_id),
        Response::Error { .. }
    ));
    let Response::Stats(after) = client.request(&Request::Stats).expect("stats after") else {
        panic!("expected stats response");
    };
    assert_eq!(after.graph_cache_entries, 0);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn disconnect_drops_staged_sync_without_publishing_it() {
    let fixture = Fixture::started();
    let mut disconnected = fixture.client();
    disconnected.handshake().expect("handshake");
    let (sync_id, _) = begin_sync(&mut disconnected, &[("dropped.js", "export dropped\n")]);
    assert!(matches!(
        stage_sync(&mut disconnected, sync_id, "dropped.js", "export dropped\n"),
        Response::Ok
    ));
    drop(disconnected);

    let mut fresh = fixture.client();
    fresh.handshake().expect("fresh handshake");
    let (_sync_id, paths) = begin_sync(&mut fresh, &[("dropped.js", "export dropped\n")]);
    assert_eq!(paths, ["dropped.js"]);
}

#[test]
fn request_and_response_round_trip_over_a_socket() {
    let fixture = Fixture::started();
    let mut client = fixture.client();

    let (pid, _) = client.handshake().expect("handshake");
    assert_eq!(pid, std::process::id(), "the daemon reports its own pid");

    let set = client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "a.js".into(),
            text: "export a\nexport b\nconst hidden = 1\n".into(),
        })
        .expect("set source");
    assert!(matches!(set, Response::Ok), "got {set:?}");

    let Response::Checked(summary) = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: "a.js".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("check")
    else {
        panic!("expected a module summary");
    };
    assert_eq!(summary.path, "a.js");
    assert_eq!(summary.exports, vec!["a".to_string(), "b".to_string()]);
    assert_ne!(summary.implementation_hash, summary.interface_hash);

    let Response::Project(project) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project")
    else {
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
fn interpreted_run_uses_a_snapshot_without_blocking_other_clients() {
    let fixture = Fixture::started();
    let source_path = fixture.dir.join("concurrent.js");
    std::fs::write(&source_path, "console.log('disk');\n").expect("write graph entry");
    let source_path = std::fs::canonicalize(source_path).expect("canonical graph entry");
    let path = source_path.to_string_lossy().into_owned();
    let project_root = fixture.dir.to_string_lossy().into_owned();
    let mut setup = fixture.client();
    setup.handshake().expect("handshake");
    setup
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: "console.log('old');\n".into(),
        })
        .expect("set initial source");
    let Response::Checked(initial) = setup
        .request(&Request::Check {
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
        })
        .expect("check initial source")
    else {
        panic!("expected initial checked response");
    };

    let (entered_rx, gate) = install_run_gate(&path);
    let socket = fixture.socket.clone();
    let run_path = path.clone();
    let run_root = project_root.clone();
    let blocked = std::thread::spawn(move || {
        let mut client = Client::connect(&socket).expect("connect run client");
        client.handshake().expect("run handshake");
        client
            .request(&Request::Run {
                stream_output: false,
                node: false,
                wall_clock: false,
                steps: StepBudget::Unmetered,
                source_view: SourceView(0),
                path: run_path,
                project_root: run_root,
                args: Vec::new(),
            })
            .expect("blocked run")
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("run reached execution gate");

    // This second client proves the execution gate is beyond the engine mutex: all three
    // requests complete while the first run remains blocked.
    let mut editor = fixture.client();
    editor.handshake().expect("editor handshake");
    let edit = editor
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: "console.log('new');\n".into(),
        })
        .expect("edit source while run is blocked");
    assert!(matches!(edit, Response::Ok), "got {edit:?}");
    let Response::Stats(stats) = editor.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(stats.revision, 2);
    let Response::Checked(summary) = editor
        .request(&Request::Check {
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
        })
        .expect("check")
    else {
        panic!("expected checked response");
    };
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);
    assert_ne!(summary.implementation_hash, initial.implementation_hash);

    gate.release();
    let response =
        within(Duration::from_secs(5), move || blocked.join()).expect("blocked run must finish");
    let Response::Ran(first) = response else {
        panic!("expected run response, got {response:?}");
    };
    assert_eq!(first.stdout, "old\n");
    assert_eq!(first.revision, 1, "run reports its prepared snapshot");

    // A later run should prepare against the edited revision and see the new source, while the
    // already-prepared run above stays on the old bytecode.
    let Response::Ran(second) = editor
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path,
            project_root,
            args: Vec::new(),
        })
        .expect("later run")
    else {
        panic!("expected later run response");
    };
    assert_eq!(second.stdout, "new\n");
    assert_eq!(second.revision, 2);
}

#[test]
fn in_flight_graph_run_keeps_old_snapshot_while_a_later_run_patches_it() {
    let fixture = Fixture::empty();
    let dependency = fixture.dir.join("snapshot-dependency.js");
    let entry = fixture.dir.join("snapshot-entry.js");
    std::fs::write(
        &dependency,
        "console.log('old'); export const answer = 42;\n",
    )
    .expect("write dependency");
    std::fs::write(
        &entry,
        "import { answer } from './snapshot-dependency.js'; console.log(answer);\n",
    )
    .expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut setup = Client::connect(&fixture.socket).expect("connect");
    setup.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let dependency = std::fs::canonicalize(dependency).expect("canonical dependency");
    let entry = std::fs::canonicalize(entry).expect("canonical entry");
    for path in [&dependency, &entry] {
        setup
            .request(&Request::SetSource {
                source_view: SourceView(0),
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("set source");
    }
    let initial_view = setup.source_view();
    let entry_path = entry.to_string_lossy().into_owned();
    let dependency_path = dependency.to_string_lossy().into_owned();

    // This gate is keyed by the old request's spelling. A later request uses the equivalent
    // relative spelling, allowing it to prepare and execute a replacement graph while the old
    // immutable Arc remains blocked in execution.
    let (entered_rx, gate) = install_run_gate(&entry_path);
    let socket = fixture.socket.clone();
    let old_entry = entry_path.clone();
    let old_root = root.clone();
    let blocked = std::thread::spawn(move || {
        let mut client = Client::connect(&socket).expect("connect run client");
        client.source_view = initial_view;
        client.handshake().expect("run handshake");
        client
            .request(&Request::Run {
                stream_output: false,
                node: false,
                wall_clock: false,
                steps: StepBudget::Unmetered,
                source_view: initial_view,
                path: old_entry,
                project_root: old_root,
                args: Vec::new(),
            })
            .expect("blocked graph run")
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("old graph reached execution gate");

    setup
        .request(&Request::SetSource {
            source_view: initial_view,
            path: dependency_path,
            text: "console.log('new'); export const answer = 7;\n".into(),
        })
        .expect("edit dependency while old graph is in flight");
    let Response::Ran(new) = setup
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: setup.source_view(),
            path: "./snapshot-entry.js".into(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("run patched graph")
    else {
        panic!("expected patched graph run");
    };
    assert!(new.succeeded, "{:?}", new.diagnostics);
    assert_eq!(new.stdout, "new\n7\n");

    gate.release();
    let response =
        within(Duration::from_secs(5), move || blocked.join()).expect("old graph run must finish");
    let Response::Ran(old) = response else {
        panic!("expected old graph run response, got {response:?}");
    };
    assert!(old.succeeded, "{:?}", old.diagnostics);
    assert_eq!(old.stdout, "old\n42\n");
    assert_eq!(old.revision, 2, "old run reports its prepared snapshot");
    assert_eq!(new.revision, 3, "patched run reports the edited snapshot");

    let Response::Stats(stats) = setup.request(&Request::Stats).expect("graph stats") else {
        panic!("expected stats response");
    };
    assert_eq!(stats.graph_cache_hits, 1);
    assert_eq!(stats.graph_cache_partial_hits, 1);
    assert_eq!(stats.graph_cache_misses, 1);
    assert_eq!(stats.graph_cache_entries, 1);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn graph_preparation_releases_state_lock_and_rejects_stale_snapshot() {
    let fixture = Fixture::empty();
    let dependency = fixture.dir.join("gated-dependency.js");
    let entry = fixture.dir.join("gated-entry.js");
    std::fs::write(
        &dependency,
        "console.log('old'); export const answer = 42;\n",
    )
    .expect("write dependency");
    std::fs::write(
        &entry,
        "import { answer } from './gated-dependency.js'; console.log(answer);\n",
    )
    .expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut setup = Client::connect(&fixture.socket).expect("connect");
    setup.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let dependency = std::fs::canonicalize(dependency).expect("canonical dependency");
    let entry = std::fs::canonicalize(entry).expect("canonical entry");
    for path in [&dependency, &entry] {
        setup
            .request(&Request::SetSource {
                source_view: SourceView(0),
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("set source");
    }
    let initial_view = setup.source_view();
    let dependency_path = dependency.to_string_lossy().into_owned();
    let entry_path = entry.to_string_lossy().into_owned();
    let unrelated_path = fixture
        .dir
        .join("gated-unrelated.js")
        .to_string_lossy()
        .into_owned();
    let (entered_rx, gate) = install_graph_gate(&dependency_path);

    let socket = fixture.socket.clone();
    let run_path = entry_path.clone();
    let run_root = root.clone();
    let blocked = std::thread::spawn(move || {
        let mut client = Client::connect(&socket).expect("connect run client");
        client.source_view = initial_view;
        client.handshake().expect("run handshake");
        client.raw_request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: initial_view,
            path: run_path,
            project_root: run_root,
            args: Vec::new(),
        })
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("graph preparation reached frontend gate");

    let socket = fixture.socket.clone();
    let update_path = unrelated_path;
    let updater = std::thread::spawn(move || {
        let mut client = Client::connect(&socket).expect("connect update client");
        client.source_view = initial_view;
        client.handshake().expect("update handshake");
        let response = client.request(&Request::SetSource {
            source_view: initial_view,
            path: update_path,
            text: "console.log('unrelated');\n".into(),
        });
        (client, response)
    });
    let (mut updater, update_response) = within(Duration::from_secs(5), move || updater.join())
        .expect("SetSource must proceed while graph preparation is gated");
    assert!(matches!(
        update_response.expect("update request"),
        Response::Ok
    ));

    gate.release();
    let stale = within(Duration::from_secs(5), move || blocked.join())
        .expect("stale graph preparation must finish");
    let stale = stale.expect("stale run response");
    assert!(
        matches!(stale, Response::StaleSourceView { .. }),
        "{stale:?}"
    );
    let Response::Stats(stats) = updater.request(&Request::Stats).expect("stats") else {
        panic!("expected stats response");
    };
    assert_eq!(stats.graph_cache_entries, 0, "stale graph was not cached");

    let updated_view = updater.source_view();
    let Response::Ran(fresh) = updater
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: updated_view,
            path: entry_path,
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("fresh run")
    else {
        panic!("expected fresh run response");
    };
    assert_eq!(fresh.stdout, "old\n42\n");
    let Response::Stats(stats) = updater.request(&Request::Stats).expect("fresh stats") else {
        panic!("expected stats response");
    };
    assert_eq!(stats.graph_cache_entries, 1);
    assert_eq!(stats.graph_cache_modules, 2);

    let updated_view = updater.source_view();
    updater
        .request(&Request::SetSource {
            source_view: updated_view,
            path: dependency_path,
            text: "console.log('newer'); export const answer = 8;\n".into(),
        })
        .expect("second edit");
    let updated_view = updater.source_view();
    let Response::Ran(fresh) = updater
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: updated_view,
            path: std::fs::canonicalize(fixture.dir.join("gated-entry.js"))
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("partial fresh run")
    else {
        panic!("expected partial fresh run response");
    };
    assert_eq!(fresh.stdout, "newer\n8\n");
    let Response::Stats(stats) = updater.request(&Request::Stats).expect("partial stats") else {
        panic!("expected stats response");
    };
    assert_eq!(
        stats.graph_cache_partial_hits, 1,
        "source revisions were captured"
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn prepared_single_file_run_keeps_its_program_and_revision_after_an_edit() {
    let fixture = Fixture::empty();
    let path = "unsaved-buffer.js".to_owned();
    let mut engine = StubEngine::default();
    let file = engine.intern(&path);
    engine.set_source(file, "console.log('old');\n".into());
    let prepared = prepare_run(
        &mut engine,
        None,
        path,
        Vec::new(),
        fixture.dir.to_string_lossy().into_owned(),
        false,
    )
    .expect("prepare unsaved buffer");

    engine.set_source(file, "console.log('new');\n".into());
    let Response::Ran(summary) =
        execute_prepared_run(prepared, false, false, StepBudget::Unmetered)
    else {
        panic!("expected run summary");
    };
    assert_eq!(summary.stdout, "old\n");
    assert_eq!(summary.revision, 1);
    assert_eq!(engine.revision().0, 2);
}

#[test]
fn conformance_execution_does_not_block_query_clients() {
    let fixture = Fixture::started();
    let corpus = fixture.dir.join("concurrent-corpus");
    std::fs::create_dir_all(&corpus).expect("create corpus");
    std::fs::write(corpus.join("one.js"), "console.log('ok');\n").expect("write test program");
    std::fs::write(
        corpus.join("manifest.toml"),
        r#"[[test]]
id = "fixture/one"
source = "one.js"
expected_stdout = "ok\n"
"#,
    )
    .expect("write manifest");
    let root = corpus.to_string_lossy().into_owned();
    let (entered_rx, gate) = install_test_gate(&root);

    let socket = fixture.socket.clone();
    let test_root = root.clone();
    let blocked = std::thread::spawn(move || {
        let mut client = Client::connect(&socket).expect("connect test client");
        client.handshake().expect("test handshake");
        client
            .request(&Request::Test {
                root: test_root,
                mode: TestMode::Parse,
                changed_root: None,
                changed_paths: None,
                previously_failed: Vec::new(),
            })
            .expect("blocked test request")
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("test reached execution gate");

    let mut editor = fixture.client();
    editor.handshake().expect("editor handshake");
    let edit = editor
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "while-testing.js".into(),
            text: "export const answer = 42;\n".into(),
        })
        .expect("set source while tests are blocked");
    assert!(matches!(edit, Response::Ok), "got {edit:?}");
    let Response::Stats(stats) = editor.request(&Request::Stats).expect("stats") else {
        panic!("expected stats response");
    };
    assert_eq!(stats.revision, 1);

    gate.release();
    let response =
        within(Duration::from_secs(5), move || blocked.join()).expect("test request must finish");
    let Response::Tested(summary) = response else {
        panic!("expected tested response, got {response:?}");
    };
    assert_eq!(summary.discovered, 1);
    assert_eq!(summary.selected, 1);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.unsupported, 0);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.cases.len(), 1);
    assert_eq!(summary.cases[0].id, "fixture/one");
    assert_eq!(summary.cases[0].variant, "sloppy");
    assert_eq!(summary.cases[0].outcome, TestOutcome::Pass);
    assert!(summary.reasons.is_empty());
}

#[test]
fn native_run_round_trips_through_the_daemon_and_reuses_artifacts() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    let source_path = fixture.dir.join("src/native.js");
    let path = source_path.to_string_lossy().into_owned();
    let project_root = fixture.dir.to_string_lossy().into_owned();
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: "console.log('native daemon', 6 * 7);".to_owned(),
        })
        .expect("upload native source");

    let request = Request::NativeRun {
        stream_output: false,
        source_view: SourceView(0),
        path,
        project_root,
        profile: NativeProfile::Dev,
        node: false,
        wall_clock: false,
        static_link: false,
        pgo_generate: None,
        pgo_use: None,
        args: Vec::new(),
        semantics: NativeSemantics::Js,
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
    let expected_output = fixture.dir.join(".kiln/bin/src").join(if cfg!(windows) {
        "native.js.exe"
    } else {
        "native.js"
    });
    assert_eq!(first.build.output_path, expected_output.to_string_lossy());
    assert!(expected_output.is_file());
    assert_eq!(
        first.scalar_value,
        Some(kiln_proto::ScalarValueSummary::Undefined)
    );
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

#[test]
fn revision_pinned_native_build_rejects_stale_source_before_publication() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    let source_path = fixture.dir.join("snapshot-native.js");
    let path = source_path.to_string_lossy().into_owned();
    let project_root = fixture.dir.to_string_lossy().into_owned();
    let output = fixture.dir.join("snapshot-native");
    std::fs::write(&output, b"existing artifact").expect("write existing artifact");

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: "console.log('first snapshot');".to_owned(),
        })
        .expect("upload first source");
    let Response::Ran(first) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
            args: Vec::new(),
        })
        .expect("run first source")
    else {
        panic!("expected interpreted run");
    };

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: "console.log('second snapshot');".to_owned(),
        })
        .expect("upload second source");
    let Response::Ran(second) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
            args: Vec::new(),
        })
        .expect("run second source")
    else {
        panic!("expected interpreted run");
    };
    assert!(second.revision > first.revision);

    let stale = client
        .request(&Request::NativeBuildAtRevision {
            node: false,
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
            output: Some(output.to_string_lossy().into_owned()),
            profile: NativeProfile::Dev,
            expected_revision: first.revision,
            explain: false,
            static_link: false,
            pgo_generate: None,
            pgo_use: None,
        })
        .expect("stale native build response");
    assert!(
        matches!(stale, Response::Stale { current_revision } if current_revision == second.revision),
        "expected stale revision {}, got {stale:?}",
        second.revision,
    );
    assert_eq!(
        std::fs::read(&output).expect("read existing artifact"),
        b"existing artifact",
        "a stale request must not replace the published executable"
    );

    let response = client
        .request(&Request::NativeBuildAtRevision {
            node: false,
            source_view: SourceView(0),
            path,
            project_root,
            output: Some(output.to_string_lossy().into_owned()),
            profile: NativeProfile::Dev,
            expected_revision: second.revision,
            explain: false,
            static_link: false,
            pgo_generate: None,
            pgo_use: None,
        })
        .expect("current native build response");
    let Response::NativeBuilt(summary) = response else {
        if let Response::Error { ref message } = response
            && message.contains("no supported C compiler")
        {
            eprintln!("skipping revision-pinned native integration: {message}");
            return;
        }
        panic!("expected current native build, got {response:?}");
    };
    assert_eq!(summary.output_path, output.to_string_lossy());
    assert_ne!(
        std::fs::read(&output).expect("read published executable"),
        b"existing artifact"
    );
    let temp_prefix = ".kiln-snapshot-native-";
    assert!(
        std::fs::read_dir(&fixture.dir)
            .expect("read fixture")
            .all(|entry| !entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .starts_with(temp_prefix)),
        "snapshot temporary outputs must be cleaned after publication"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn production_daemon_native_shared_build_returns_summary_and_cache_hits() {
    let fixture = Fixture::empty();
    let source_dir = fixture.dir.join("src");
    std::fs::create_dir(&source_dir).expect("create shared source directory");
    let source = source_dir.join("native-shared.js");
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
        source_view: SourceView(0),
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
        .join(".kiln/lib/src")
        .join(format!("native-shared.js.{extension}"));
    assert_eq!(first.output_path, expected_output.to_string_lossy());
    let loader = Path::new(&first.loader_path);
    assert_eq!(
        loader.parent(),
        Some(
            expected_output
                .parent()
                .unwrap()
                .join(".kiln-loaders")
                .as_path()
        )
    );
    assert!(
        loader
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("kiln-"))
    );
    assert!(loader.is_file());
    assert_eq!(
        std::fs::read(loader).expect("read canonical loader"),
        std::fs::read(&expected_output).expect("read requested shared library")
    );
    let expected_header = expected_output.with_extension("h");
    assert_eq!(first.header_path, expected_header.to_string_lossy());
    let header = std::fs::read_to_string(&first.header_path).expect("read shared build header");
    assert!(header.contains(&format!("int {}(", first.module_symbol)));
    assert_eq!(
        first.module_api_symbol,
        format!("{}_api_v2", first.module_symbol)
    );
    assert!(header.contains(&format!("{}(", first.module_api_symbol)));
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
    assert_eq!(second.loader_path, first.loader_path);
    assert_eq!(second.header_path, first.header_path);
    assert!(second.generated_c_cache_hit);
    assert!(second.object_cache_hit);
    assert!(second.library_cache_hit);
    assert_eq!(second.library_bytes, first.library_bytes);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn native_shared_relative_alias_converges_after_one_stale_retry() {
    let fixture = Fixture::empty();
    let source = fixture.dir.join("relative-native-shared.js");
    std::fs::write(&source, "console.log('disk');\n").expect("write source");
    let root = std::fs::canonicalize(&fixture.dir).expect("canonical root");
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "relative-native-shared.js".into(),
            text: "console.log('overlay');\n".into(),
        })
        .expect("set relative overlay");
    let request = Request::NativeSharedBuild {
        source_view: SourceView(0),
        path: "relative-native-shared.js".into(),
        project_root: root.to_string_lossy().into_owned(),
        output: Some(
            root.join("relative-native-shared.so")
                .to_string_lossy()
                .into_owned(),
        ),
        profile: NativeProfile::Dev,
    };

    let mut response = client.raw_request(&request).expect("first shared build");
    if matches!(response, Response::StaleSourceView { .. }) {
        response = client.raw_request(&request).expect("retry shared build");
    }
    match response {
        Response::NativeSharedBuilt(_) => {}
        Response::Error { ref message } if message.contains("no supported C compiler") => {}
        Response::StaleSourceView { current } => {
            panic!(
                "relative native shared build remained stale at view {}",
                current.0
            )
        }
        other => panic!("unexpected shared build response: {other:?}"),
    }

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
            source_view: SourceView(0),
            path: "embed.h".into(),
            text: "console.log('embedded');\n".into(),
        })
        .expect("upload header-named source");
    let response = client
        .request(&Request::NativeSharedBuild {
            source_view: SourceView(0),
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
            source_view: SourceView(0),
            path: "embed.js".into(),
            text: "console.log('embedded');\n".into(),
        })
        .expect("upload source");
    let response = client
        .request(&Request::NativeSharedBuild {
            source_view: SourceView(0),
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
                source_view: SourceView(0),
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("upload source");
    }
    let request = Request::NativeSharedBuild {
        source_view: SourceView(0),
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
        .join(format!("shared-entry.js.{extension}"));
    assert_eq!(first.output_path, expected_output.to_string_lossy());
    let loader = Path::new(&first.loader_path);
    assert_eq!(
        loader.parent(),
        Some(
            expected_output
                .parent()
                .unwrap()
                .join(".kiln-loaders")
                .as_path()
        )
    );
    assert!(loader.is_file());
    assert_eq!(
        std::fs::read(loader).expect("read graph canonical loader"),
        std::fs::read(&expected_output).expect("read requested graph library")
    );
    let expected_header = expected_output.with_extension("h");
    assert_eq!(first.header_path, expected_header.to_string_lossy());
    assert!(
        !first.module_symbol.is_empty(),
        "wrapper symbol is reported"
    );
    assert_eq!(
        first.module_api_symbol,
        format!("{}_api_v2", first.module_symbol)
    );
    let header = std::fs::read_to_string(&first.header_path).expect("read shared graph header");
    assert!(header.contains(&format!("int {}(", first.module_symbol)));
    assert!(header.contains(&format!("{}(", first.module_api_symbol)));
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
    assert_eq!(second.loader_path, first.loader_path);
    assert_eq!(second.header_path, first.header_path);
    assert_eq!(second.module_symbol, first.module_symbol);
    assert_eq!(second.module_api_symbol, first.module_api_symbol);
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
                source_view: SourceView(0),
                path: format!("m{index}.js"),
                text: format!("export e{index}\n"),
            })
            .expect("set source");
        assert!(matches!(response, Response::Ok));
    }
    let Response::Project(project) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project")
    else {
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
            protocol: PROTOCOL_VERSION - 1,
            build: crate::build_identity(),
        })
        .expect("the daemon answers before hanging up");
    let Response::Error { message } = response else {
        panic!("a version-skewed client must be refused, got {response:?}");
    };
    assert!(
        message.contains(&(PROTOCOL_VERSION - 1).to_string())
            && message.contains(&PROTOCOL_VERSION.to_string()),
        "the error must name both versions: {message}"
    );

    // Refusing the request but continuing to serve the connection would let a skewed client go
    // on to issue requests we cannot interpret.
    let after = client.request(&Request::Project {
        source_view: SourceView(0),
    });
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
            source_view: SourceView(0),
            path: "boom.js".into(),
            text: POISON.into(),
        })
        .expect("set source");

    // The default panic hook prints a backtrace notice here; that is the daemon logging a bug,
    // and is expected noise in this test's output.
    let response = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: "boom.js".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("the daemon answers rather than dying");
    let Response::Error { message } = response else {
        panic!("expected an error response, got {response:?}");
    };
    assert!(message.contains("internal error"), "{message}");

    // Same connection, then a fresh one: neither the client's session nor the daemon is lost.
    assert!(matches!(
        client
            .request(&Request::Project {
                source_view: SourceView(0)
            })
            .expect("still serving"),
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
        client.handshake().expect("handshake").0
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
        build: crate::build_identity(),
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
            source_view: SourceView(0),
            path: module.to_string_lossy().into_owned(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("check")
    else {
        panic!("expected a module summary");
    };
    assert_eq!(summary.exports, vec!["fromDisk".to_string()]);
}

#[test]
fn a_relative_pushed_buffer_wins_over_disk_during_a_canonical_check() {
    // The editor case: the buffer is unsaved, so re-reading the file would silently check the
    // wrong text.
    let fixture = Fixture::started();
    let module = fixture.dir.join("buffered.js");
    std::fs::write(&module, "export saved\n").expect("write fixture module");
    let path = std::fs::canonicalize(&module)
        .expect("canonical module")
        .to_string_lossy()
        .into_owned();
    let project_root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical project root")
        .to_string_lossy()
        .into_owned();

    let mut client = fixture.client();
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "buffered.js".into(),
            text: "export unsaved\n".into(),
        })
        .expect("set source");

    let Response::Checked(summary) = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path,
            project_root,
        })
        .expect("check")
    else {
        panic!("expected a module summary");
    };
    assert_eq!(summary.exports, vec!["unsaved".to_string()]);
    let Response::Project(project) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project")
    else {
        panic!("expected a project summary");
    };
    assert_eq!(project.modules, 1, "one file must have one active spelling");
}

#[test]
fn checking_a_missing_file_is_an_error_not_a_panic() {
    let fixture = Fixture::started();
    let missing = fixture.dir.join("nope.js").to_string_lossy().into_owned();

    let mut client = fixture.client();
    let response = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: missing.clone(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("check");
    assert!(
        matches!(&response, Response::Error { message } if message.contains(&missing)),
        "got {response:?}"
    );
    assert!(matches!(
        client
            .request(&Request::Project {
                source_view: SourceView(0)
            })
            .expect("still serving"),
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
    assert_eq!(direct.file_name(), Some(Path::new("s").as_os_str()));
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
            source_view: SourceView(0),
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
            source_view: SourceView(0),
            path: "real.ts".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
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
        client
            .request(&Request::Project {
                source_view: SourceView(0)
            })
            .expect("project"),
        Response::Project(_)
    ));
    client
        .request(&Request::ResetStats)
        .expect("reset counters");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
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
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project after edit");
    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(stats.parse_computed, 1);
    assert_eq!(stats.interface_computed, 1);
    assert_eq!(stats.project_computed, 0);
    // Interface, source-check diagnostics, and project results are unchanged.
    assert_eq!(stats.early_cutoff, 3);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "real.ts".into(),
            text: "export const answer: = ;".into(),
        })
        .expect("set malformed source");
    let Response::Checked(summary) = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: "real.ts".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
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
fn production_check_reports_the_graph_resolved_star_surface() {
    let fixture = Fixture::empty();
    let left = fixture.dir.join("left.js");
    let right = fixture.dir.join("right.js");
    let broken = fixture.dir.join("broken.js");
    let barrel = fixture.dir.join("barrel.js");
    std::fs::write(&left, "export const answer = 42;\n").expect("write left origin");
    std::fs::write(&right, "export const answer = 42;\n").expect("write right origin");
    std::fs::write(&broken, "export { missing };\n").expect("write broken dependency");
    std::fs::write(&barrel, "export * from './left.js';\n").expect("write barrel");
    let left = std::fs::canonicalize(left).expect("canonical left origin");
    let right = std::fs::canonicalize(right).expect("canonical right origin");
    let broken = std::fs::canonicalize(broken).expect("canonical broken dependency");
    let barrel = std::fs::canonicalize(barrel).expect("canonical barrel");
    let project_root = std::fs::canonicalize(&fixture.dir).expect("canonical project root");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    for (path, text) in [
        (&left, "export const answer = 42;\n"),
        (&right, "export const answer = 42;\n"),
        (&broken, "export { missing };\n"),
        (&barrel, "export * from './left.js';\n"),
    ] {
        client
            .request(&Request::SetSource {
                source_view: SourceView(0),
                path: path.to_string_lossy().into_owned(),
                text: text.to_owned(),
            })
            .expect("synchronize graph source");
    }

    let Response::Checked(summary) = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: barrel.to_string_lossy().into_owned(),
            project_root: project_root.to_string_lossy().into_owned(),
        })
        .expect("check barrel")
    else {
        panic!("expected checked response");
    };
    assert!(
        summary.exports.is_empty(),
        "stars are not local declarations"
    );
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);
    assert_eq!(summary.resolved_exports, Some(vec!["answer".to_owned()]));

    let Response::Project(before) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project before")
    else {
        panic!("expected project summary");
    };
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: barrel.to_string_lossy().into_owned(),
            text: "export * from './right.js';\n".to_owned(),
        })
        .expect("switch star target");
    let Response::Project(after) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("project after")
    else {
        panic!("expected project summary");
    };
    assert_eq!(
        (after.modules, after.exports),
        (before.modules, before.exports)
    );
    assert_ne!(after.interface_hash, before.interface_hash);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: barrel.to_string_lossy().into_owned(),
            text: "export * from './broken.js';\n".to_owned(),
        })
        .expect("point barrel at broken dependency");
    let Response::Checked(failed) = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: barrel.to_string_lossy().into_owned(),
            project_root: project_root.to_string_lossy().into_owned(),
        })
        .expect("check broken dependency")
    else {
        panic!("expected checked response");
    };
    assert_eq!(failed.resolved_exports, None);
    assert!(
        failed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path.as_deref() == broken.to_str()),
        "dependency diagnostics must retain their module path: {:?}",
        failed.diagnostics
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_daemon_executes_a_node_builtin_module_with_explicit_capability() {
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");

    let path = "run.mjs".to_owned();
    let text = "import { createServer } from 'node:http'; console.log(typeof createServer);\n";
    std::fs::write(fixture.dir.join(&path), text).expect("write Node source");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: text.into(),
        })
        .expect("set Node source");
    let response = client
        .request(&Request::Run {
            stream_output: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            path,
            project_root: fixture.dir.to_string_lossy().into_owned(),
            node: true,
            wall_clock: false,
            args: Vec::new(),
        })
        .expect("run Node module");
    let Response::Ran(summary) = response else {
        panic!("expected run response, got {response:?}");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(summary.stdout, "function\n");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_wall_clock_reads_system_time_across_immediate_turns() {
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let epoch_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("current epoch")
            .as_secs_f64()
            * 1000.0
    };
    for (path, prefix) in [
        ("clock.cjs", ""),
        ("clock-module.mjs", "import 'node:timers';\n"),
    ] {
        let text = format!(
            "{prefix}console.log(Date.now());\n\
            let count = 0; function next() {{\n\
                if (++count < 250) setImmediate(next); else console.log(Date.now());\n\
            }} setImmediate(next);\n"
        );
        std::fs::write(fixture.dir.join(path), &text).expect("write clock source");
        client
            .request(&Request::SetSource {
                source_view: client.source_view(),
                path: path.to_owned(),
                text,
            })
            .expect("set clock source");
        for wall_clock in [true, false] {
            let before = epoch_ms().floor();
            let response = client
                .request(&Request::Run {
                    stream_output: false,
                    steps: StepBudget::Unmetered,
                    source_view: client.source_view(),
                    path: path.to_owned(),
                    project_root: fixture.dir.to_string_lossy().into_owned(),
                    node: true,
                    wall_clock,
                    args: Vec::new(),
                })
                .expect("run clock source");
            let after = epoch_ms().ceil();
            let Response::Ran(summary) = response else {
                panic!("expected run response, got {response:?}");
            };
            assert!(summary.succeeded, "{:?}", summary.diagnostics);
            let readings: Vec<f64> = summary
                .stdout
                .lines()
                .map(|line| line.parse().expect("numeric epoch time"))
                .collect();
            assert_eq!(readings.len(), 2);
            if wall_clock {
                for reading in readings {
                    assert_eq!(reading.fract(), 0.0, "Date.now has integral milliseconds");
                    assert!(
                        reading >= before && reading <= after,
                        "{path}: epoch {reading} outside {before}..={after}"
                    );
                }
            } else {
                assert_eq!(
                    readings,
                    [0.0, 0.0],
                    "immediates do not advance virtual time"
                );
            }
        }
    }
    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn wall_clock_runs_without_the_node_adapter() {
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let epoch_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("current epoch")
            .as_secs_f64()
            * 1000.0
    };
    // A module (graph path) and a script (single-program path), both without `node`.
    for (path, text) in [
        (
            "plain-clock.mjs",
            "export {};\nconsole.log(Date.now());\nsetTimeout(() => console.log(Date.now()), 40);\n",
        ),
        (
            "plain-clock.js",
            "console.log(Date.now());\nsetTimeout(function () { console.log(Date.now()); }, 40);\n",
        ),
    ] {
        std::fs::write(fixture.dir.join(path), text).expect("write clock source");
        client
            .request(&Request::SetSource {
                source_view: client.source_view(),
                path: path.to_owned(),
                text: text.to_owned(),
            })
            .expect("set clock source");
        let before = epoch_ms().floor();
        let response = client
            .request(&Request::Run {
                stream_output: false,
                steps: StepBudget::Unmetered,
                source_view: client.source_view(),
                path: path.to_owned(),
                project_root: fixture.dir.to_string_lossy().into_owned(),
                node: false,
                wall_clock: true,
                args: Vec::new(),
            })
            .expect("run clock source");
        let after = epoch_ms().ceil();
        let Response::Ran(summary) = response else {
            panic!("expected run response, got {response:?}");
        };
        assert!(summary.succeeded, "{path}: {:?}", summary.diagnostics);
        let readings: Vec<f64> = summary
            .stdout
            .lines()
            .map(|line| line.parse().expect("numeric epoch time"))
            .collect();
        assert_eq!(readings.len(), 2, "{path}: {}", summary.stdout);
        for reading in &readings {
            assert!(
                *reading >= before && *reading <= after,
                "{path}: epoch {reading} outside {before}..={after}"
            );
        }
        assert!(readings[1] - readings[0] >= 30.0, "{path}: {readings:?}");
    }
    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[cfg(unix)]
#[test]
fn node_launch_evaluates_preloads_and_runtime_created_esm_in_one_module_realm() {
    let fixture = Fixture::empty();
    let entry = fixture.dir.join("entry.mjs");
    let preload = fixture.dir.join("preload.mjs");
    let generated = fixture.dir.join("generated.mjs");
    let entry_source = "import { execFileSync } from 'node:child_process';\n\
        globalThis.__startup.push('entry');\n\
        const target = './generated.mjs';\n\
        const generated = await import(target);\n\
        let codegen = 'allowed';\n\
        try { new Function('return 1')(); } catch { codegen = 'blocked'; }\n\
        const child = execFileSync('/bin/sh', ['-c', 'printf child'], { encoding: 'utf8' });\n\
        console.log(globalThis.__startup.join(','), generated.value, codegen, child, process.execArgv.join(','));\n";
    let preload_source = "globalThis.__startup = ['preload'];\n";
    std::fs::write(&entry, entry_source).expect("write entry");
    std::fs::write(&preload, preload_source).expect("write preload");
    // This is deliberately absent from the synchronized source snapshot. The runtime loader must
    // resolve it from the dynamic import's referrer and load it into the existing realm.
    std::fs::write(&generated, "export const value = 42;\n").expect("write runtime module");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    for (path, text) in [(&entry, entry_source), (&preload, preload_source)] {
        let path = std::fs::canonicalize(path).expect("canonical source path");
        client
            .request(&Request::SetSource {
                source_view: client.source_view(),
                path: path.to_string_lossy().into_owned(),
                text: text.to_owned(),
            })
            .expect("synchronize launch source");
    }
    let response = client
        .request(&Request::NodeRun {
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root,
            args: Vec::new(),
            preloads: vec!["./preload.mjs".to_owned()],
            exec_argv: vec![
                "--disallow-code-generation-from-strings".to_owned(),
                "--import".to_owned(),
                "./preload.mjs".to_owned(),
            ],
            disallow_string_code_generation: true,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            stream_output: false,
        })
        .expect("execute Node-style launch");
    let Response::Ran(summary) = response else {
        panic!("expected run response, got {response:?}");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(
        summary.stdout,
        "preload,entry 42 blocked child --disallow-code-generation-from-strings,--import,./preload.mjs\n"
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[cfg(unix)]
#[test]
fn runtime_file_url_queries_keep_distinct_source_graphs_after_a_file_changes() {
    let fixture = Fixture::empty();
    let entry = fixture.dir.join("entry.mjs");
    let generated = fixture.dir.join("generated.mjs");
    let entry_source = "import { unlinkSync, writeFileSync } from 'node:fs';\n\
        import { fileURLToPath } from 'node:url';\n\
        const url = new URL('./generated.mjs', import.meta.url).href;\n\
        const first = await import(url + '?one');\n\
        writeFileSync(fileURLToPath(url), \"export const value = 'two';\\n\");\n\
        const second = await import(url + '?two');\n\
        unlinkSync(fileURLToPath(url));\n\
        const firstAgain = await import(url + '?one');\n\
        console.log(first.value, second.value, firstAgain.value, firstAgain === first);\n";
    std::fs::write(&entry, entry_source).expect("write runtime-query entry");
    std::fs::write(&generated, "export const value = 'one';\n")
        .expect("write runtime-query target");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let path = std::fs::canonicalize(&entry).expect("canonical entry");
    client
        .request(&Request::SetSource {
            source_view: client.source_view(),
            path: path.to_string_lossy().into_owned(),
            text: entry_source.to_owned(),
        })
        .expect("synchronize runtime-query entry");
    let response = client
        .request(&Request::NodeRun {
            path: path.to_string_lossy().into_owned(),
            project_root: root,
            args: Vec::new(),
            preloads: Vec::new(),
            exec_argv: Vec::new(),
            disallow_string_code_generation: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            stream_output: false,
        })
        .expect("execute runtime-query imports");
    let Response::Ran(summary) = response else {
        panic!("expected run response, got {response:?}");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(summary.stdout, "one two one true\n");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[cfg(unix)]
#[test]
fn node_run_graph_cache_tracks_ordered_preloads_and_their_dependencies() {
    fn run(client: &mut Client, path: &str, root: &str, preloads: Vec<String>) -> String {
        let response = client
            .request(&Request::NodeRun {
                path: path.to_owned(),
                project_root: root.to_owned(),
                args: Vec::new(),
                preloads,
                exec_argv: Vec::new(),
                disallow_string_code_generation: false,
                wall_clock: false,
                steps: StepBudget::Unmetered,
                source_view: client.source_view(),
                stream_output: false,
            })
            .expect("run with startup preloads");
        let Response::Ran(summary) = response else {
            panic!("expected run response, got {response:?}");
        };
        assert!(summary.succeeded, "{:?}", summary.diagnostics);
        summary.stdout
    }

    let fixture = Fixture::empty();
    let entry = fixture.dir.join("entry.mjs");
    let first = fixture.dir.join("preload-first.mjs");
    let second = fixture.dir.join("preload-second.mjs");
    let dependency = fixture.dir.join("shared.mjs");
    let entry_source = "console.log(globalThis.order.join(','));\n";
    let first_source = "globalThis.order ??= []; import { version } from './shared.mjs'; globalThis.order.push('first:' + version);\n";
    let second_source = "globalThis.order ??= []; import { version } from './shared.mjs'; globalThis.order.push('second:' + version);\n";
    std::fs::write(&entry, entry_source).expect("write entry");
    std::fs::write(&first, first_source).expect("write first preload");
    std::fs::write(&second, second_source).expect("write second preload");
    std::fs::write(&dependency, "export const version = 'one';\n")
        .expect("write preload dependency");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical root")
        .to_string_lossy()
        .into_owned();
    let entry_path = std::fs::canonicalize(entry).expect("canonical entry");
    for (path, text) in [
        (entry_path.clone(), entry_source),
        (
            std::fs::canonicalize(first).expect("canonical first preload"),
            first_source,
        ),
        (
            std::fs::canonicalize(second).expect("canonical second preload"),
            second_source,
        ),
        (
            std::fs::canonicalize(&dependency).expect("canonical preload dependency"),
            "export const version = 'one';\n",
        ),
    ] {
        client
            .request(&Request::SetSource {
                source_view: client.source_view(),
                path: path.to_string_lossy().into_owned(),
                text: text.to_owned(),
            })
            .expect("synchronize startup graph source");
    }
    let entry_path = entry_path.to_string_lossy().into_owned();
    let ordered = vec![
        "./preload-first.mjs".to_owned(),
        "./preload-second.mjs".to_owned(),
    ];
    assert_eq!(
        run(&mut client, &entry_path, &root, ordered.clone()),
        "first:one,second:one\n"
    );
    assert_eq!(
        run(&mut client, &entry_path, &root, ordered.clone()),
        "first:one,second:one\n"
    );
    let Response::Stats(after_hit) = client.request(&Request::Stats).expect("cache hit stats")
    else {
        panic!("expected stats response");
    };
    assert_eq!(after_hit.graph_cache_hits, 1);

    client
        .request(&Request::SetSource {
            source_view: client.source_view(),
            path: std::fs::canonicalize(&dependency)
                .expect("canonical preload dependency")
                .to_string_lossy()
                .into_owned(),
            text: "export const version = 'two';\n".to_owned(),
        })
        .expect("edit preload dependency");
    assert_eq!(
        run(&mut client, &entry_path, &root, ordered.clone()),
        "first:two,second:two\n"
    );
    let Response::Stats(after_edit) = client.request(&Request::Stats).expect("edit stats") else {
        panic!("expected stats response");
    };
    assert_eq!(after_edit.graph_cache_hits, 2);
    assert_eq!(after_edit.graph_cache_partial_hits, 1);

    let reversed = vec![
        "./preload-second.mjs".to_owned(),
        "./preload-first.mjs".to_owned(),
    ];
    assert_eq!(
        run(&mut client, &entry_path, &root, reversed),
        "second:two,first:two\n"
    );
    let Response::Stats(after_reversal) = client.request(&Request::Stats).expect("order stats")
    else {
        panic!("expected stats response");
    };
    assert_eq!(after_reversal.graph_cache_hits, 2);
    assert_eq!(after_reversal.graph_cache_entries, 2);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn production_daemon_forwards_run_arguments_to_process_argv() {
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");

    let path = "argv.mjs".to_owned();
    let text = "console.log(JSON.stringify(process.argv.slice(2)));\n";
    std::fs::write(fixture.dir.join(&path), text).expect("write source");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: text.into(),
        })
        .expect("set source");
    let response = client
        .request(&Request::Run {
            stream_output: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            path,
            project_root: fixture.dir.to_string_lossy().into_owned(),
            node: false,
            wall_clock: false,
            args: vec!["--foo".into(), "bar".into()],
        })
        .expect("run module");
    let Response::Ran(summary) = response else {
        panic!("expected run response, got {response:?}");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(summary.stdout, "[\"--foo\",\"bar\"]\n");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_daemon_does_not_evaluate_an_unused_side_effect_free_module() {
    let fixture = Fixture::empty();
    std::fs::write(
        fixture.dir.join("package.json"),
        r#"{"type":"module","sideEffects":false}"#,
    )
    .expect("write package policy");
    let dependency = fixture.dir.join("unused.mjs");
    std::fs::write(
        &dependency,
        "console.log('dead module ran'); export const unused = 1;\n",
    )
    .expect("write unused module");
    let entry = fixture.dir.join("main.mjs");
    let source = "import { unused } from './unused.mjs'; console.log('entry');\n";
    std::fs::write(&entry, source).expect("write entry");

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: entry.to_string_lossy().into_owned(),
            text: source.into(),
        })
        .expect("set entry source");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: dependency.to_string_lossy().into_owned(),
            text: "console.log('dead module ran'); export const unused = 1;\n".into(),
        })
        .expect("set dependency source");

    let Response::Ran(summary) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            path: entry.to_string_lossy().into_owned(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
            args: Vec::new(),
        })
        .expect("run shaken graph")
    else {
        panic!("expected run response");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(summary.stdout, "entry\n");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_daemon_compiles_direct_eval_at_runtime() {
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");

    let path = "eval.js".to_owned();
    let text = "console.log(eval('40 + 2'));\n";
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: text.into(),
        })
        .expect("set source");
    let Response::Ran(summary) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            path,
            project_root: fixture.dir.to_string_lossy().into_owned(),
            args: Vec::new(),
        })
        .expect("run eval")
    else {
        panic!("expected run response");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(summary.stdout, "42\n");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("the accept loop must not panic");
}

#[test]
fn production_daemon_compiles_function_constructor_at_runtime() {
    let fixture = Fixture::empty();
    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut client = Client::connect(&fixture.socket).expect("connect");
    client.handshake().expect("handshake");

    let path = "function.js".to_owned();
    let text = "console.log(Function('return 6')());\n";
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: path.clone(),
            text: text.into(),
        })
        .expect("set source");
    let Response::Ran(summary) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: client.source_view(),
            path,
            project_root: fixture.dir.to_string_lossy().into_owned(),
            args: Vec::new(),
        })
        .expect("run Function")
    else {
        panic!("expected run response");
    };
    assert!(summary.succeeded, "{:?}", summary.diagnostics);
    assert_eq!(summary.stdout, "6\n");

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
            source_view: SourceView(0),
            path: disk_module.to_string_lossy().into_owned(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
        })
        .expect("production check reads disk")
    else {
        panic!("expected checked response");
    };
    assert_eq!(disk_summary.exports, ["loaded"]);

    let path = "run.ts".to_string();
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
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
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: path.clone(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
            args: Vec::new(),
        })
        .expect("run")
    else {
        panic!("expected run response");
    };
    assert!(first.succeeded, "{:?}", first.diagnostics);
    assert_eq!(first.stdout, "inner 9\nshadow 7\nkiln 3\n");
    assert_eq!(
        first.scalar_value,
        Some(kiln_proto::ScalarValueSummary::Undefined)
    );
    assert!(first.instructions > 0);

    client
        .request(&Request::ResetStats)
        .expect("reset counters");
    let Response::Ran(second) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: path.clone(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
            args: Vec::new(),
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
            source_view: SourceView(0),
            path: path.clone(),
            // Any construct lowering still refuses works here; phased dynamic import is one that
            // parses cleanly, so the diagnostic comes from lowering with a span attached rather
            // than from the parser.
            text: "const value = import.defer(\"./x.js\");\n".into(),
        })
        .expect("set unsupported source");
    let Response::Ran(failed) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path,
            project_root: fixture.dir.to_string_lossy().into_owned(),
            args: Vec::new(),
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
                && diagnostic.message.contains("phased dynamic import")),
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
                source_view: SourceView(0),
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("set source");
    }

    let Response::Checked(summary) = client
        .request(&Request::Check {
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
        })
        .expect("check graph")
    else {
        panic!("expected checked response");
    };
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);
    let Response::Stats(after_check) = client.request(&Request::Stats).expect("check stats") else {
        panic!("expected stats response");
    };
    assert_eq!(after_check.graph_cache_hits, 0);
    assert_eq!(after_check.graph_cache_misses, 1);
    assert_eq!(after_check.graph_cache_entries, 1);
    assert_eq!(after_check.graph_cache_modules, 2);

    let response = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("run graph");
    let Response::Ran(first) = response else {
        panic!("expected run response");
    };
    assert!(first.succeeded, "{:?}", first.diagnostics);
    assert_eq!(first.stdout, "dependency\n42\n");
    assert!(first.instructions > 0);

    let Response::Stats(after_run) = client.request(&Request::Stats).expect("run stats") else {
        panic!("expected stats response");
    };
    assert_eq!(after_run.graph_cache_hits, 1);
    assert_eq!(after_run.graph_cache_misses, 1);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: std::fs::canonicalize(&dependency)
                .expect("canonical dependency")
                .to_string_lossy()
                .into_owned(),
            text: "console.log('updated dependency'); export const answer = 7;\n".into(),
        })
        .expect("set body-only edit");
    let Response::Ran(updated) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("run body-only edit")
    else {
        panic!("expected run response");
    };
    assert!(updated.succeeded, "{:?}", updated.diagnostics);
    assert_eq!(updated.stdout, "updated dependency\n7\n");
    let Response::Stats(after_update) = client.request(&Request::Stats).expect("update stats")
    else {
        panic!("expected stats response");
    };
    assert_eq!(after_update.graph_cache_hits, 2);
    assert_eq!(after_update.graph_cache_partial_hits, 1);
    assert_eq!(after_update.graph_cache_misses, 1);
    assert_eq!(after_update.graph_cache_evictions, 1);

    let failed_edit = "export const =;\n";
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: std::fs::canonicalize(&dependency)
                .expect("canonical dependency")
                .to_string_lossy()
                .into_owned(),
            text: failed_edit.into(),
        })
        .expect("set invalid body edit");
    let Response::Ran(failed) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("invalid body edit is structured")
    else {
        panic!("expected run response");
    };
    assert!(!failed.succeeded);
    let Response::Stats(after_failed_edit) =
        client.request(&Request::Stats).expect("failed edit stats")
    else {
        panic!("expected stats response");
    };
    assert_eq!(after_failed_edit.graph_cache_entries, 1);
    assert_eq!(
        after_failed_edit.graph_cache_misses, 2,
        "a failed one-module patch compile is observable as a miss"
    );
    assert_eq!(after_failed_edit.graph_cache_partial_hits, 1);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: std::fs::canonicalize(&dependency)
                .expect("canonical dependency")
                .to_string_lossy()
                .into_owned(),
            text: "console.log('updated dependency'); export const answer = 7;\n".into(),
        })
        .expect("restore valid body edit");
    let Response::Ran(restored) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("restored body edit")
    else {
        panic!("expected run response");
    };
    assert!(restored.succeeded, "{:?}", restored.diagnostics);
    assert_eq!(restored.stdout, "updated dependency\n7\n");

    client.request(&Request::ResetStats).expect("reset stats");
    let Response::Ran(second) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("warm graph run")
    else {
        panic!("expected run response");
    };
    assert_eq!(second.stdout, updated.stdout);
    let Response::Stats(stats) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats response");
    };
    assert_eq!(stats.parse_computed, 0);
    assert_eq!(stats.ir_computed, 0);
    assert_eq!(stats.graph_cache_hits, 1);
    assert_eq!(stats.graph_cache_partial_hits, 0);
    assert_eq!(stats.graph_cache_misses, 0);
    assert_eq!(stats.graph_cache_entries, 1);
    assert_eq!(stats.graph_cache_modules, 2);

    std::fs::write(
        &dependency,
        "import './entry.js'; export const answer = 42;\n",
    )
    .expect("write cycle");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: std::fs::canonicalize(&dependency)
                .expect("canonical dependency")
                .to_string_lossy()
                .into_owned(),
            text: "import './entry.js'; export const answer = 42;\n".into(),
        })
        .expect("set cycle");
    let Response::Ran(cycle) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            project_root: root,
            args: Vec::new(),
        })
        .expect("cycle is structured")
    else {
        panic!("expected run response");
    };
    // The dependency now imports the entry back. That is a legal cyclic graph: the back edge
    // carries no binding, so the dependency still evaluates first and the entry still reads
    // `answer` from a module that has finished. It runs, and it prints what it printed before
    // the cycle existed.
    assert!(cycle.succeeded, "{:?}", cycle.diagnostics);
    // The rewritten dependency no longer logs, so only the entry's own line remains -- and the
    // entry still saw the value the dependency exported, which is the point.
    assert_eq!(cycle.stdout, "42\n");
    let Response::Stats(after_cycle) = client.request(&Request::Stats).expect("cycle stats") else {
        panic!("expected stats response");
    };
    assert_eq!(after_cycle.graph_cache_misses, 1);
    assert_eq!(
        after_cycle.graph_cache_entries, 1,
        "failed rebuild retains old snapshot"
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
                .request(&Request::SetSource {
                    source_view: SourceView(0),
                    path,
                    text,
                })
                .expect("upload source"),
            Response::Ok
        ));
    }

    let project_root = std::fs::canonicalize(&fixture.dir)
        .expect("canonical project root")
        .to_string_lossy()
        .into_owned();
    let Response::Ran(first) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: entry_path.clone(),
            project_root: project_root.clone(),
            args: Vec::new(),
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
                hash: kiln_proto::source_digest(&entry_source),
            }],
            resolutions: kiln_proto::ResolutionMap::new(project_root.clone(), Vec::new())
                .expect("canonical empty resolution map"),
        })
        .expect("sync authoritative view");
    let Response::Needed { sync_id, paths } = sync else {
        panic!("expected Needed response");
    };
    assert!(paths.is_empty());
    let Response::Synced { .. } = client
        .request(&Request::CommitSync { sync_id })
        .expect("commit authoritative view")
    else {
        panic!("expected Synced response");
    };

    let Response::Ran(second) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: entry_path,
            project_root,
            args: Vec::new(),
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
    let canonical_entry = std::fs::canonicalize(&entry)
        .expect("canonical entry")
        .to_string_lossy()
        .into_owned();
    let mut stable_file = None;

    for ((source, expected), expected_revision) in [
        ("console.log('first');\n", "first\n"),
        ("console.log('second');\n", "second\n"),
    ]
    .into_iter()
    .zip([3, 6])
    {
        client
            .request(&Request::SetSource {
                source_view: SourceView(0),
                path: "overlay.js".into(),
                text: source.into(),
            })
            .expect("set relative overlay");
        let Response::Ran(result) = client
            .request(&Request::Run {
                stream_output: false,
                node: false,
                wall_clock: false,
                steps: StepBudget::Unmetered,
                source_view: SourceView(0),
                path: canonical_entry.clone(),
                project_root: root.clone(),
                args: Vec::new(),
            })
            .expect("run overlay")
        else {
            panic!("expected run response");
        };
        assert!(result.succeeded, "{:?}", result.diagnostics);
        assert_eq!(result.stdout, expected);
        assert_eq!(result.revision, expected_revision);
        if let Some(file) = stable_file {
            assert_eq!(result.file, file, "the canonical FileId must stay stable");
        } else {
            stable_file = Some(result.file);
        }

        let Response::Project(project) = client
            .request(&Request::Project {
                source_view: SourceView(0),
            })
            .expect("read canonical project")
        else {
            panic!("expected project response");
        };
        assert_eq!(
            project.modules, 1,
            "one physical module must not remain active under two spellings"
        );

        let Response::Ran(repeated) = client
            .request(&Request::Run {
                stream_output: false,
                node: false,
                wall_clock: false,
                steps: StepBudget::Unmetered,
                source_view: SourceView(0),
                path: "overlay.js".into(),
                project_root: root.clone(),
                args: Vec::new(),
            })
            .expect("repeat run without another upload")
        else {
            panic!("expected repeated run response");
        };
        assert_eq!(repeated.stdout, expected);
        assert_eq!(repeated.file, result.file);
        assert_eq!(
            repeated.revision, result.revision,
            "identity normalization must settle after one preparation"
        );
    }
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "missing/../overlay.js".into(),
            text: "console.log('lexical overlay');\n".into(),
        })
        .expect("set unresolved lexical overlay");
    let Response::Ran(lexical) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: canonical_entry.clone(),
            project_root: root.clone(),
            args: Vec::new(),
        })
        .expect("run lexical overlay")
    else {
        panic!("expected lexical overlay response");
    };
    assert_eq!(lexical.stdout, "lexical overlay\n");
    let Response::Project(project) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("read project")
    else {
        panic!("expected project response");
    };
    assert_eq!(project.modules, 1);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "overlay.js".into(),
            text: "console.log('older alias');\n".into(),
        })
        .expect("set older relative overlay");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: canonical_entry,
            text: "console.log('newer canonical');\n".into(),
        })
        .expect("set newer canonical overlay");
    let Response::Ran(newest) = client
        .request(&Request::Run {
            stream_output: false,
            node: false,
            wall_clock: false,
            steps: StepBudget::Unmetered,
            source_view: SourceView(0),
            path: "overlay.js".into(),
            project_root: root,
            args: Vec::new(),
        })
        .expect("run newest alias")
    else {
        panic!("expected newest run response");
    };
    assert_eq!(newest.stdout, "newer canonical\n");
    assert_eq!(Some(newest.file), stable_file);

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

#[test]
fn revision_pinned_native_build_accepts_relative_alias_reconciliation() {
    let fixture = Fixture::started();
    let source = fixture.dir.join("relative-snapshot.js");
    std::fs::write(&source, "console.log('disk');\n").expect("write disk source");
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "relative-snapshot.js".into(),
            text: "console.log('overlay');\n".into(),
        })
        .expect("set relative snapshot");
    let Response::Stats(before) = client.request(&Request::Stats).expect("read revision") else {
        panic!("expected stats response");
    };

    let response = client
        .raw_request(&Request::NativeBuildAtRevision {
            node: false,
            source_view: SourceView(0),
            path: "relative-snapshot.js".into(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
            output: Some(
                fixture
                    .dir
                    .join("relative-snapshot")
                    .to_string_lossy()
                    .into_owned(),
            ),
            profile: NativeProfile::Dev,
            expected_revision: before.revision,
            explain: false,
            static_link: false,
            pgo_generate: None,
            pgo_use: None,
        })
        .expect("revision-pinned relative build");
    match response {
        Response::NativeBuilt(_) => {}
        Response::Error { message } if message.contains("no supported C compiler") => {}
        Response::Stale { current_revision } => panic!(
            "alias reconciliation advanced to {current_revision} while holding the preparation \
             lock and must become the build snapshot"
        ),
        Response::StaleSourceView { .. } => {}
        other => panic!("unexpected relative native build response: {other:?}"),
    }

    let Response::Project(project) = client
        .request(&Request::Project {
            source_view: SourceView(0),
        })
        .expect("read project")
    else {
        panic!("expected project response");
    };
    assert_eq!(project.modules, 1);
}

/// A cache sweep is the one request that can delete a file, so it is worth proving over a real
/// socket that `--dry-run` reaches the daemon as a dry run and that the answer is a report rather
/// than an error, without a source view anywhere in the exchange.
#[test]
fn cache_gc_sweeps_over_the_socket_without_disturbing_the_source_set() {
    let fixture = Fixture::started();
    let shard = fixture.dir.join(".kiln/cache/generated-c/aa");
    std::fs::create_dir_all(&shard).expect("create cache shard");
    let record = shard.join("a".repeat(64));
    std::fs::write(&record, vec![b'x'; 2048]).expect("seed a cache record");

    let mut client = fixture.client();
    client.handshake().expect("handshake");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: "gc.js".into(),
            text: "export const a = 1;\n".into(),
        })
        .expect("set source");
    let Response::Stats(before) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };

    let response = client
        .raw_request(&Request::CacheGc {
            project_root: fixture.dir.to_string_lossy().into_owned(),
            dry_run: true,
        })
        .expect("cache gc");
    match response {
        Response::CacheSwept(summary) => {
            assert!(summary.dry_run);
            assert_eq!(summary.layers.len(), 1, "{summary:?}");
            assert_eq!(summary.layers[0].records_scanned, 1, "{summary:?}");
            assert_eq!(
                summary.cache_root,
                fixture.dir.join(".kiln/cache").to_string_lossy()
            );
        }
        other => panic!("unexpected cache gc response: {other:?}"),
    }

    assert!(record.exists(), "a dry run must not delete a record");
    let Response::Stats(after) = client.request(&Request::Stats).expect("stats") else {
        panic!("expected stats");
    };
    assert_eq!(
        after.revision, before.revision,
        "a sweep is not a source event"
    );
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
                source_view: SourceView(0),
                path: path.to_string_lossy().into_owned(),
                text: std::fs::read_to_string(path).expect("read source"),
            })
            .expect("set source");
    }
    let request = Request::NativeRun {
        stream_output: false,
        source_view: SourceView(0),
        path: canonical_entry.to_string_lossy().into_owned(),
        project_root: root,
        profile: NativeProfile::Dev,
        node: false,
        wall_clock: false,
        static_link: false,
        pgo_generate: None,
        pgo_use: None,
        args: Vec::new(),
        semantics: NativeSemantics::Js,
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
    assert_eq!(
        first.scalar_value,
        Some(kiln_proto::ScalarValueSummary::Undefined)
    );
    let Response::Stats(after_first) = client.request(&Request::Stats).expect("first native stats")
    else {
        panic!("expected stats response");
    };
    assert_eq!(after_first.graph_cache_hits, 0);
    assert_eq!(after_first.graph_cache_partial_hits, 0);
    assert_eq!(after_first.graph_cache_misses, 1);
    assert_eq!(after_first.graph_cache_entries, 1);
    assert_eq!(after_first.graph_cache_modules, 2);

    let Response::NativeRan(second) = client.request(&request).expect("cached native graph run")
    else {
        panic!("expected cached native run");
    };
    assert_eq!(second.stdout, first.stdout);
    assert!(second.build.generated_c_cache_hit);
    assert!(second.build.app_object_cache_hit);
    let Response::Stats(after_second) = client
        .request(&Request::Stats)
        .expect("second native stats")
    else {
        panic!("expected stats response");
    };
    assert_eq!(after_second.graph_cache_hits, 1);
    assert_eq!(after_second.graph_cache_partial_hits, 0);
    assert_eq!(after_second.graph_cache_misses, 1);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: canonical_dependency.to_string_lossy().into_owned(),
            text: "import './native-entry.js'; export const answer = 42;\n".into(),
        })
        .expect("set cycle");
    // A cycle whose back edge carries no binding compiles natively too: the launcher still has
    // every provider's export array in hand before the module that reads it.
    let Response::NativeRan(cyclic) = client.request(&request).expect("cyclic native run") else {
        panic!("expected a native run for a cycle nothing reads forward across");
    };
    assert_eq!(cyclic.stdout, "42\n");

    // A cycle whose back edge *does* carry a binding now runs with the ordinary live-binding
    // semantics: the dependency reads `seed` before the entry reaches its initializer, so the
    // program reports the runtime TDZ ReferenceError. It must not be rejected as an unsupported
    // cycle during graph construction.
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: std::fs::canonicalize(&entry)
                .expect("canonical entry")
                .to_string_lossy()
                .into_owned(),
            text: "export const seed = 42;\n\
                   import { answer } from './native-dependency.js';\n\
                   console.log(answer);\n"
                .into(),
        })
        .expect("set forward cycle entry");
    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
            path: canonical_dependency.to_string_lossy().into_owned(),
            text: "import { seed } from './native-entry.js';\n\
                   export const answer = seed;\n"
                .into(),
        })
        .expect("set forward cycle");
    let response = client.request(&request).expect("forward cycle response");
    let Response::NativeRan(forward_cycle) = response else {
        panic!("expected the native graph to execute the forward cycle, got {response:?}");
    };
    assert_eq!(forward_cycle.exit_code, Some(1));
    assert_eq!(forward_cycle.stdout, "");
    assert!(
        forward_cycle.stderr.contains("ReferenceError")
            && forward_cycle.stderr.contains("before its declaration"),
        "the runtime error must report the uninitialized live binding: {}",
        forward_cycle.stderr
    );

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
}

/// `process.argv[2..]` is where a program's own arguments live, in node and in a kiln binary
/// alike. `kiln run` accepts none, so the interpreted command line is exactly the two entries
/// node always puts in front of them -- never one, which would move the entry to index 0 and
/// shift every position a program reads by one.
#[test]
fn interpreted_argv_names_the_interpreter_then_the_entry() {
    let argv = interpreted_argv("/projects/app/main.mjs", &[]);
    assert_eq!(argv.len(), 2);
    assert!(!argv[0].is_empty());
    assert_eq!(argv[1], "/projects/app/main.mjs");

    let argv_with_args = interpreted_argv(
        "/projects/app/main.mjs",
        &["--flag".to_string(), "arg".to_string()],
    );
    assert_eq!(argv_with_args.len(), 4);
    assert_eq!(argv_with_args[1], "/projects/app/main.mjs");
    assert_eq!(argv_with_args[2], "--flag");
    assert_eq!(argv_with_args[3], "arg");
}

fn serve_with_idle(fixture: &Fixture, idle: Option<Duration>) -> JoinHandle<()> {
    let daemon = Daemon::bind_with(&fixture.socket, StubEngine::default())
        .expect("bind")
        .with_idle_timeout(idle);
    std::thread::spawn(move || daemon.serve())
}

fn wait_until_finished(serving: &JoinHandle<()>, limit: Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if serving.is_finished() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    serving.is_finished()
}

#[test]
fn an_idle_daemon_exits_and_unlinks_its_socket() {
    let fixture = Fixture::empty();
    let serving = serve_with_idle(&fixture, Some(Duration::from_millis(300)));
    // Served a request first: idleness is measured from the last client, not from startup.
    fixture.client().handshake().expect("handshake");
    assert!(
        wait_until_finished(&serving, Duration::from_secs(10)),
        "an idle daemon must exit"
    );
    serving.join().expect("clean exit");
    assert!(!fixture.socket.exists(), "the socket must be unlinked");
    assert!(
        Client::connect(&fixture.socket).is_err(),
        "nothing may answer after the exit"
    );
}

#[test]
fn a_daemon_that_never_had_a_client_still_goes_idle() {
    let fixture = Fixture::empty();
    let serving = serve_with_idle(&fixture, Some(Duration::from_millis(200)));
    assert!(wait_until_finished(&serving, Duration::from_secs(10)));
    assert!(!fixture.socket.exists());
}

#[test]
fn an_open_connection_keeps_the_daemon_alive() {
    // A `watch` session holds one connection open between saves and sends nothing for as long as
    // the files do not change.
    let fixture = Fixture::empty();
    let idle = Duration::from_millis(300);
    let serving = serve_with_idle(&fixture, Some(idle));
    let mut client = fixture.client();
    client.handshake().expect("handshake");

    std::thread::sleep(idle * 4);
    assert!(!serving.is_finished(), "exited under an open connection");
    client.handshake().expect("the daemon still answers");

    // The clock starts when the last client leaves, not before.
    drop(client);
    assert!(
        wait_until_finished(&serving, Duration::from_secs(10)),
        "the daemon must go idle once the last client is gone"
    );
    assert!(!fixture.socket.exists());
}

#[test]
fn a_new_request_restarts_the_idle_clock() {
    let fixture = Fixture::empty();
    let idle = Duration::from_millis(600);
    let serving = serve_with_idle(&fixture, Some(idle));
    for _ in 0..4 {
        std::thread::sleep(idle / 3);
        fixture.client().handshake().expect("still serving");
    }
    assert!(!serving.is_finished());
    assert!(wait_until_finished(&serving, Duration::from_secs(10)));
}

#[test]
fn a_zero_idle_period_disables_the_exit() {
    let mut fixture = Fixture::empty();
    fixture.serving = Some(serve_with_idle(&fixture, Some(Duration::ZERO)));
    fixture.stop = None;
    std::thread::sleep(Duration::from_millis(400));
    assert!(!fixture.serving.as_ref().unwrap().is_finished());
    fixture.client().handshake().expect("still serving");
    // `Fixture::stop` needs a handle; ask for the shutdown the way a client would.
    fixture
        .client()
        .raw_request(&Request::Shutdown)
        .expect("shutdown");
    fixture.stop();
}

/// `check_source_types` over one program in a directory with no configuration, plus any
/// project files the program should see.
fn check_program(
    source: &str,
    project: &[(&str, &str)],
    auxiliary: &[(&str, &str)],
) -> kiln_hir::source_types::CheckResult {
    use std::sync::Arc;
    let entry = "/project/main.ts";
    let modules: Vec<_> = std::iter::once((entry, source))
        .chain(project.iter().copied())
        .map(|(path, text)| {
            (
                path.to_owned(),
                Arc::new(kiln_frontend_oxc::parse(path, text)),
            )
        })
        .collect();
    let auxiliary: Vec<(String, Arc<[u8]>)> = auxiliary
        .iter()
        .map(|(path, text)| ((*path).to_owned(), Arc::from(text.as_bytes())))
        .collect();
    crate::server::check_source_types(entry, &modules, &auxiliary)
}

fn messages(result: &kiln_hir::source_types::CheckResult) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(|d| format!("{:?}: {}", d.code, d.message))
        .collect()
}

#[test]
fn source_check_without_a_config_knows_the_globals_kiln_provides() {
    let result = check_program(
        r#"
        const xs: number[] = [1, 2, 3];
        setTimeout(() => console.log(xs.map((x) => x * 2)), 10);
        const text: Promise<string> = fetch("http://example.com").then((r) => r.text());
        const url = new URL("http://example.com");
        console.log(process.argv.length, Buffer.from("x").length, structuredClone(url).href, text);
        "#,
        &[],
        &[],
    );
    assert!(result.diagnostics.is_empty(), "{:?}", messages(&result));
}

#[test]
fn source_check_knows_node_modules_and_still_reports_real_errors() {
    let result = check_program(
        r#"
        import { readFileSync } from "node:fs";
        import path from "node:path";
        const ok: string = readFileSync("x", "utf8");
        const bad: number = readFileSync("x", "utf8");
        console.log(path.join("a", "b"), ok, bad);
        "#,
        &[],
        &[],
    );
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        vec![Some(2322)],
        "{:?}",
        messages(&result)
    );
}

#[test]
fn source_check_knows_kiln_extensions_and_modules() {
    let result = check_program(
        r#"
        import { run } from "kiln:process";
        const doubled: number[] = [1, 2].parallelMap((x) => x * 2);
        const total: number = new Float64Array(4).parallelReduce((a, v) => a + v, (a, b) => a + b, 0);
        Kiln.serve({ fetch: () => new Response("hi"), port: 0 });
        Bun.serve((request) => new Response(request.url));
        run("echo", ["hi"]).then((r) => console.log(r.exitCode, doubled, total));
        const wrong: string = [1].parallelMap((x) => x);
        "#,
        &[],
        &[],
    );
    assert_eq!(
        result
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        vec![Some(2322)],
        "{:?}",
        messages(&result)
    );
}

#[test]
fn source_check_prefers_the_projects_own_node_types() {
    // The project's @types/node declares one name only. If the embedded copy were loaded too,
    // `process` would resolve; it must not, and the project's own declaration must.
    let project = [(
        "/project/node_modules/@types/node/index.d.ts",
        "declare const ONLY_IN_PROJECT_NODE_TYPES: 1;",
    )];
    let result = check_program(
        "const a: 1 = ONLY_IN_PROJECT_NODE_TYPES; process.exit(0);",
        &project,
        &[],
    );
    let codes: Vec<_> = result.diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(codes, vec![Some(2591)], "{:?}", messages(&result));
}

#[test]
fn source_check_respects_a_projects_config_but_keeps_the_builtin_declarations() {
    // `strict` is off, so assigning null to a number is fine; the config says nothing of Node.
    let result = check_program(
        r#"
        import { readFileSync } from "node:fs";
        const n: number = null;
        console.log(n, readFileSync("x", "utf8"), [1].parallelMap((x) => x));
        "#,
        &[],
        &[(
            "/project/tsconfig.json",
            r#"{"compilerOptions":{"strict":false,"lib":["es2020"]}}"#,
        )],
    );
    assert!(result.diagnostics.is_empty(), "{:?}", messages(&result));
    let strict = check_program(
        "const n: number = null; console.log(n);",
        &[],
        &[(
            "/project/tsconfig.json",
            r#"{"compilerOptions":{"strict":true}}"#,
        )],
    );
    assert_eq!(
        strict
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        vec![Some(2322)],
        "{:?}",
        messages(&strict)
    );
}

/// Every member `KILN_EXTENSIONS` declares is declared in `@kiln/types`, and the declarations
/// carry no extension the registry lacks -- so adding an extension without a declaration (or the
/// reverse) fails here.
#[test]
fn kiln_types_declare_exactly_the_registered_extensions() {
    use kiln_builtin_surface::KILN_EXTENSIONS;
    let file = |name: &str| {
        kiln_type_checker::KILN_LIBRARIES
            .iter()
            .find(|(path, _)| *path == format!("kiln/{name}"))
            .unwrap_or_else(|| panic!("kiln/{name} is not embedded"))
            .1
    };
    let parallel = file("parallel.d.ts");
    let globals = file("globals.d.ts");
    // `interface Name<..> { ... }` bodies, keyed by name.
    let body = |name: &str| -> &str {
        let at = parallel
            .find(&format!("\ninterface {name}"))
            .unwrap_or_else(|| panic!("parallel.d.ts has no interface {name}"));
        let rest = &parallel[at + 1..];
        &rest[..rest.find("\n}").expect("interface body ends")]
    };
    const TYPED: [&str; 11] = [
        "Int8Array",
        "Uint8Array",
        "Uint8ClampedArray",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "Float32Array",
        "Float64Array",
        "BigInt64Array",
        "BigUint64Array",
    ];
    let mut registered_parallel = std::collections::BTreeSet::new();
    for extension in KILN_EXTENSIONS {
        let key = extension.key.property_key();
        let declared = |interface: &str| {
            let text = body(interface);
            text.contains(&format!("\n  {key}(")) || text.contains(&format!("\n  {key}<"))
        };
        match extension.owner {
            "Array.prototype" => {
                registered_parallel.insert(key);
                assert!(declared("Array"), "Array.prototype.{key} is not declared");
                if key != "parallelSort" {
                    assert!(
                        declared("ReadonlyArray"),
                        "ReadonlyArray.{key} is not declared"
                    );
                }
            }
            "%TypedArray%.prototype" => {
                registered_parallel.insert(key);
                for typed in TYPED {
                    assert!(declared(typed), "{typed}.{key} is not declared");
                }
            }
            "globalThis" => assert!(
                globals.contains(&format!("declare var {key}:")),
                "global {key} is not declared"
            ),
            other => {
                panic!("extension owner {other} has no declaration rule; add one in kiln-types")
            }
        }
    }
    let declared_parallel: std::collections::BTreeSet<_> = parallel
        .lines()
        .filter_map(|line| line.strip_prefix("  parallel"))
        .filter_map(|rest| rest.split(['(', '<']).next())
        .map(|name| format!("parallel{name}"))
        .collect();
    let registered_parallel: std::collections::BTreeSet<_> = registered_parallel
        .iter()
        .map(|k| (*k).to_owned())
        .collect();
    assert_eq!(declared_parallel, registered_parallel);
}

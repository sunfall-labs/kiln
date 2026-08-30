//! End-to-end tests for the daemon, driven over a real unix socket.
//!
//! They run against [`StubEngine`] rather than the production database: the contract the server
//! is written against is [`QueryEngine`], and a stub is the only way to provoke the failure modes
//! that matter here - notably a query that panics, which a correct engine never does.

use crate::dispatch::{execute_prepared_run, install_run_gate, install_test_gate, prepare_run};
use crate::framing::MAX_MESSAGE_BYTES;
use crate::{Client as RawClient, Daemon, DaemonError, ShutdownHandle};
use kiln_db::{FileId, Interface, Parsed, Project, QueryEngine, Revision, Stats};
use kiln_proto::{
    NativeProfile, PROTOCOL_VERSION, PackageResolution, Request, ResolutionMap, Response,
    SourceHash, SourceView, SyncId, TestMode, TestOutcome,
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

/// Test clients keep the current protocol source view automatically. This lets the older daemon
/// tests focus on query behavior while still exercising the v17 CAS contract.
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

    fn handshake(&mut self) -> std::io::Result<u32> {
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

fn begin_sync(client: &mut Client, files: &[(&str, &str)]) -> (SyncId, Vec<String>) {
    begin_sync_with_resolutions(
        client,
        files,
        ResolutionMap::new("/", Vec::new()).expect("empty resolution map"),
    )
}

fn begin_sync_with_resolutions(
    client: &mut Client,
    files: &[(&str, &str)],
    resolutions: ResolutionMap,
) -> (SyncId, Vec<String>) {
    let response = client
        .raw_request(&Request::Sync {
            files: files
                .iter()
                .map(|(path, text)| SourceHash {
                    path: (*path).to_owned(),
                    hash: kiln_proto::source_digest(text),
                })
                .collect(),
            resolutions,
        })
        .expect("begin sync");
    let Response::Needed { sync_id, paths } = response else {
        panic!("expected Needed response, got {response:?}");
    };
    (sync_id, paths)
}

#[test]
fn package_resolution_is_pinned_to_the_committed_source_view() {
    let fixture = Fixture::empty();
    let package_dir = fixture.dir.join("node_modules/pkg");
    std::fs::create_dir_all(&package_dir).expect("create package directory");
    let entry = fixture.dir.join("entry.js");
    let first = package_dir.join("first.js");
    let second = package_dir.join("second.js");
    let package_json = package_dir.join("package.json");
    let entry_source = "import { value } from 'pkg'; console.log(value);\n";
    let first_source = "export const value = 'first';\n";
    let second_source = "export const value = 'second';\n";
    let first_manifest = r#"{"exports":"./first.js"}"#;
    let second_manifest = r#"{"exports":"./second.js"}"#;
    for (path, source) in [
        (&entry, entry_source),
        (&first, first_source),
        (&second, second_source),
        (&package_json, first_manifest),
    ] {
        std::fs::write(path, source).expect("write package fixture");
    }

    let entry = std::fs::canonicalize(entry).expect("canonical entry");
    let first = std::fs::canonicalize(first).expect("canonical first target");
    let second = std::fs::canonicalize(second).expect("canonical second target");
    let package_json = std::fs::canonicalize(package_json).expect("canonical manifest");
    let root = std::fs::canonicalize(&fixture.dir).expect("canonical project root");
    let entry_path = entry.to_string_lossy().into_owned();
    let first_path = first.to_string_lossy().into_owned();
    let second_path = second.to_string_lossy().into_owned();
    let manifest_path = package_json.to_string_lossy().into_owned();
    let root_path = root.to_string_lossy().into_owned();
    let files = [
        (entry_path.as_str(), entry_source),
        (first_path.as_str(), first_source),
        (second_path.as_str(), second_source),
    ];
    let resolution = |target: &str, manifest: &str| {
        ResolutionMap::new(
            root_path.clone(),
            vec![PackageResolution {
                importer: entry_path.clone(),
                specifier: "pkg".to_owned(),
                target: target.to_owned(),
                package_json: manifest_path.clone(),
                package_json_hash: kiln_proto::source_digest(manifest),
            }],
        )
        .expect("valid resolution map")
    };

    let daemon = Daemon::bind(&fixture.socket).expect("bind");
    let stop = daemon.shutdown_handle();
    let serving = std::thread::spawn(move || daemon.serve());
    let mut stale = Client::connect(&fixture.socket).expect("connect stale client");
    let mut writer = Client::connect(&fixture.socket).expect("connect writer");
    stale.handshake().expect("stale handshake");
    writer.handshake().expect("writer handshake");

    let (stale_sync, stale_needed) =
        begin_sync_with_resolutions(&mut stale, &files, resolution(&first_path, first_manifest));
    for (path, source) in &files {
        if stale_needed.iter().any(|needed| needed == path) {
            assert!(matches!(
                stage_sync(&mut stale, stale_sync, path, source),
                Response::Ok
            ));
        }
    }
    std::fs::write(&package_json, second_manifest).expect("select second package target");
    let (writer_sync, writer_needed) = begin_sync_with_resolutions(
        &mut writer,
        &files,
        resolution(&second_path, second_manifest),
    );
    for (path, source) in &files {
        if writer_needed.iter().any(|needed| needed == path) {
            assert!(matches!(
                stage_sync(&mut writer, writer_sync, path, source),
                Response::Ok
            ));
        }
    }
    assert!(matches!(
        commit_sync(&mut writer, writer_sync),
        Response::Synced {
            source_view: SourceView(1)
        }
    ));
    assert!(matches!(
        commit_sync(&mut stale, stale_sync),
        Response::StaleSourceView {
            current: SourceView(1)
        }
    ));

    // The filesystem now claims the other target, but the committed source view must continue
    // using the exact resolution table uploaded by the winning transaction.
    std::fs::write(&package_json, first_manifest).expect("mutate live package manifest");
    std::fs::remove_file(&second).expect("remove pinned target after staging its bytes");
    let Response::Ran(pinned) = writer
        .request(&Request::Run {
            path: entry_path.clone(),
            project_root: root_path.clone(),
            source_view: SourceView(0),
        })
        .expect("run pinned package graph")
    else {
        panic!("expected run response");
    };
    assert!(pinned.succeeded, "{:?}", pinned.diagnostics);
    assert_eq!(pinned.stdout, "second\n");

    let (next_sync, needed) =
        begin_sync_with_resolutions(&mut writer, &files, resolution(&first_path, first_manifest));
    assert!(needed.is_empty(), "source bytes should remain warm");
    assert!(matches!(
        commit_sync(&mut writer, next_sync),
        Response::Synced {
            source_view: SourceView(2)
        }
    ));
    let Response::Ran(re_resolved) = writer
        .request(&Request::Run {
            path: entry_path,
            project_root: root_path,
            source_view: SourceView(0),
        })
        .expect("run re-resolved package graph")
    else {
        panic!("expected run response");
    };
    assert!(re_resolved.succeeded, "{:?}", re_resolved.diagnostics);
    assert_eq!(re_resolved.stdout, "first\n");

    stop.shutdown();
    within(Duration::from_secs(5), move || serving.join()).expect("accept loop must stop");
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
                source_view: old_view,
                path: "source.js".into(),
                project_root: fixture.dir.to_string_lossy().into_owned(),
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
fn sync_rejects_noncanonical_resolution_paths() {
    let fixture = Fixture::started();
    let mut client = fixture.client();
    client.handshake().expect("handshake");
    let root = std::fs::canonicalize(&fixture.dir).expect("canonical project root");
    let importer = root.join("entry.js").to_string_lossy().into_owned();
    let target = root
        .join("node_modules/pkg/../pkg/index.js")
        .to_string_lossy()
        .into_owned();
    let response = client
        .raw_request(&Request::Sync {
            files: vec![
                SourceHash {
                    path: importer.clone(),
                    hash: kiln_proto::source_digest("import 'pkg';"),
                },
                SourceHash {
                    path: target.clone(),
                    hash: kiln_proto::source_digest("export const value = 1;"),
                },
            ],
            resolutions: ResolutionMap::new(
                root.to_string_lossy(),
                vec![PackageResolution {
                    importer,
                    specifier: "pkg".into(),
                    target,
                    package_json: root
                        .join("node_modules/pkg/package.json")
                        .to_string_lossy()
                        .into_owned(),
                    package_json_hash: kiln_proto::source_digest("{}"),
                }],
            )
            .expect("wire-canonical resolution map"),
        })
        .expect("sync response");
    let Response::Error { message } = response else {
        panic!("expected invalid resolution error, got {response:?}");
    };
    assert!(message.contains("canonical path"), "{message}");
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

    let pid = client.handshake().expect("handshake");
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
                source_view: SourceView(0),
                path: run_path,
                project_root: run_root,
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
            source_view: SourceView(0),
            path,
            project_root,
        })
        .expect("later run")
    else {
        panic!("expected later run response");
    };
    assert_eq!(second.stdout, "new\n");
    assert_eq!(second.revision, 2);
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
        path,
        fixture.dir.to_string_lossy().into_owned(),
        None,
    )
    .expect("prepare unsaved buffer");

    engine.set_source(file, "console.log('new');\n".into());
    let Response::Ran(summary) = execute_prepared_run(prepared) else {
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
        source_view: SourceView(0),
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
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
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
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
        })
        .expect("run second source")
    else {
        panic!("expected interpreted run");
    };
    assert!(second.revision > first.revision);

    let stale = client
        .request(&Request::NativeBuildAtRevision {
            source_view: SourceView(0),
            path: path.clone(),
            project_root: project_root.clone(),
            output: Some(output.to_string_lossy().into_owned()),
            profile: NativeProfile::Dev,
            expected_revision: first.revision,
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
            source_view: SourceView(0),
            path,
            project_root,
            output: Some(output.to_string_lossy().into_owned()),
            profile: NativeProfile::Dev,
            expected_revision: second.revision,
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
    assert_eq!(second.loader_path, first.loader_path);
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
    assert_eq!(stats.early_cutoff, 1);

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
            source_view: SourceView(0),
            path: path.clone(),
            project_root: fixture.dir.to_string_lossy().into_owned(),
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
            source_view: SourceView(0),
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
            source_view: SourceView(0),
            path: path.clone(),
            // Any construct lowering still refuses works here; `import.meta` is one that
            // parses cleanly, so the diagnostic comes from lowering with a span attached rather
            // than from the parser.
            text: "const value = import.meta.url;\n".into(),
        })
        .expect("set unsupported source");
    let Response::Ran(failed) = client
        .request(&Request::Run {
            source_view: SourceView(0),
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
                && diagnostic.message.contains("import.meta")),
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
            source_view: SourceView(0),
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

    let Response::Stats(after_run) = client.request(&Request::Stats).expect("run stats") else {
        panic!("expected stats response");
    };
    assert_eq!(after_run.graph_cache_hits, 1);
    assert_eq!(after_run.graph_cache_misses, 1);

    client.request(&Request::ResetStats).expect("reset stats");
    let Response::Ran(second) = client
        .request(&Request::Run {
            source_view: SourceView(0),
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
    assert_eq!(stats.graph_cache_hits, 1);
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
            source_view: SourceView(0),
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
            source_view: SourceView(0),
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
                hash: kiln_proto::source_digest(&entry_source),
            }],
            resolutions: ResolutionMap::new(&project_root, Vec::new())
                .expect("empty resolution map"),
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
            source_view: SourceView(0),
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
                source_view: SourceView(0),
                path: canonical_entry.clone(),
                project_root: root.clone(),
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
                source_view: SourceView(0),
                path: "overlay.js".into(),
                project_root: root.clone(),
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
            source_view: SourceView(0),
            path: canonical_entry.clone(),
            project_root: root.clone(),
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
            source_view: SourceView(0),
            path: "overlay.js".into(),
            project_root: root,
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
        source_view: SourceView(0),
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
    assert_eq!(
        first.scalar_value,
        Some(kiln_proto::ScalarValueSummary::Undefined)
    );

    let Response::NativeRan(second) = client.request(&request).expect("cached native graph run")
    else {
        panic!("expected cached native run");
    };
    assert_eq!(second.stdout, first.stdout);
    assert!(second.build.generated_c_cache_hit);
    assert!(second.build.app_object_cache_hit);

    client
        .request(&Request::SetSource {
            source_view: SourceView(0),
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

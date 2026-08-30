//! `--one-shot`: run the pipeline in this process and leave nothing behind.
//!
//! CI and reproducible builds must not depend on a background process that might be warm,
//! cold, or holding someone else's edits, so every command can be run against a database that
//! is created and destroyed by the invocation itself.
//!
//! The engine runs in this process, but it is still reached over a private socket in a private
//! temporary directory rather than by calling the dispatcher directly, because `kiln-daemon`
//! does not export one: `dispatch` is crate-private and `MemoEngine` is not re-exported, so a
//! direct in-process call is not available to a client that only depends on `kiln-daemon`. The
//! loopback costs a few microseconds per request against a check that takes milliseconds, and
//! it has the compensating virtue that one-shot and daemon mode exercise exactly the same
//! code path. If `kiln-daemon` later exports its dispatcher, this type shrinks to a wrapper
//! around it and nothing else in the CLI changes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use kiln_daemon::{Daemon, ShutdownHandle};
use kiln_proto::{Request, Response};

use crate::backend::Backend;
use crate::client::DaemonClient;
use crate::error::{Error, Result};

pub struct OneShot {
    client: DaemonClient,
    socket: PathBuf,
    stop: ShutdownHandle,
    serving: Option<JoinHandle<()>>,
    dir: PathBuf,
}

static NEXT_ONE_SHOT: AtomicU64 = AtomicU64::new(0);

impl OneShot {
    pub fn start() -> Result<OneShot> {
        // PID separates processes; the serial separates concurrent sessions in this process.
        // The latter matters to embedders and to parallel tests, where PID alone made one session
        // delete another's live socket directory.
        let serial = NEXT_ONE_SHOT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kiln-1s-{}-{serial}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("daemon.sock");

        let daemon = Daemon::bind(&socket).map_err(|source| {
            Error::Daemon(format!("cannot start an in-process engine: {source}"))
        })?;
        let stop = daemon.shutdown_handle();
        // Built rather than `spawn`ed, for the stack: a default 2 MiB thread overflows on an
        // ordinary project, and a stack overflow aborts the process rather than reporting.
        // See `kiln_daemon::SERVE_STACK_BYTES`.
        let serving = std::thread::Builder::new()
            .stack_size(kiln_daemon::SERVE_STACK_BYTES)
            .spawn(move || daemon.serve())
            .map_err(|source| {
                Error::Daemon(format!("cannot start an in-process engine: {source}"))
            })?;

        // `bind` already returned, so the socket is accepting: no polling needed here.
        match DaemonClient::connect(&socket).and_then(|mut client| {
            client.handshake()?;
            Ok(client)
        }) {
            Ok(client) => Ok(OneShot {
                client,
                socket,
                stop,
                serving: Some(serving),
                dir,
            }),
            Err(error) => {
                stop.shutdown();
                let _ = serving.join();
                let _ = std::fs::remove_dir_all(&dir);
                Err(error)
            }
        }
    }
}

impl Backend for OneShot {
    fn peer(&self) -> Result<Box<dyn Backend>> {
        let mut peer = DaemonClient::connect(&self.socket)?;
        peer.handshake()?;
        Ok(Box::new(peer))
    }

    fn request(&mut self, request: Request) -> Result<Response> {
        self.client.request(request)
    }
}

impl Drop for OneShot {
    fn drop(&mut self) {
        self.stop.shutdown();
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
        // The engine was private to this invocation; leaving its socket directory behind
        // would turn "reproducible" into "accumulates litter in $TMPDIR".
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{SourceResponse, SyncCommit};
    use crate::commands;
    use kiln_proto::{PackageResolution, ResolutionMap, SourceHash, source_digest};

    #[test]
    fn concurrent_one_shot_sessions_have_private_socket_directories() {
        let first = OneShot::start().expect("start first engine");
        let second = OneShot::start().expect("start second engine");
        assert_ne!(first.dir, second.dir);
        assert!(first.socket.exists());
        assert!(second.socket.exists());
    }

    #[test]
    fn concurrent_one_shot_sessions_have_private_socket_directories() {
        let first = OneShot::start().expect("start first engine");
        let second = OneShot::start().expect("start second engine");
        assert_ne!(first.dir, second.dir);
        assert!(first.socket.exists());
        assert!(second.socket.exists());
    }

    #[test]
    fn a_one_shot_run_answers_and_then_leaves_nothing_behind() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-cli-1s-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create fixture");
        let source = fixture.join("a.js");
        std::fs::write(&source, "export const a = 1;\n").expect("write source");

        let dir = {
            let mut session = OneShot::start().expect("start an in-process engine");
            let out = commands::check(&mut session, &source, &fixture)
                .expect("check")
                .text;
            assert!(out.contains("a.js  r"), "{out}");
            // A fresh database has computed nothing before, so this check is all new work.
            assert!(out.contains("recomputed      parse 1"), "{out}");
            session.dir.clone()
        };

        assert!(
            !dir.exists(),
            "{} should have been cleaned up",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_run_synchronizes_and_executes_a_package_dependency() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-cli-package-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        let package = fixture.join("node_modules/pkg");
        std::fs::create_dir_all(&package).expect("create package fixture");
        let source = fixture.join("main.js");
        std::fs::write(
            &source,
            "import { answer } from 'pkg'; console.log(answer);\n",
        )
        .expect("write entry");
        std::fs::write(package.join("package.json"), r#"{"exports":"./index.js"}"#)
            .expect("write package manifest");
        std::fs::write(package.join("index.js"), "export const answer = 42;\n")
            .expect("write package entry");

        let (output, updated) = {
            let mut session = OneShot::start().expect("start an in-process engine");
            let output = commands::run(&mut session, &source, &fixture).expect("run package graph");
            std::fs::write(package.join("index.js"), "export const answer = 43;\n")
                .expect("update package entry");
            let updated = commands::run(&mut session, &source, &fixture)
                .expect("rerun updated package graph");
            (output, updated)
        };

        assert_eq!(output.text, "42\n");
        assert_eq!(output.exit_code, 0);
        assert_eq!(updated.text, "43\n");
        assert_eq!(updated.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn daemon_replays_the_pinned_package_target_after_the_manifest_disappears() {
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-pinned-package-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        let package = fixture.join("node_modules/pkg");
        std::fs::create_dir_all(&package).expect("create package fixture");
        let source = fixture.join("main.js");
        let target = package.join("index.js");
        let manifest = package.join("package.json");
        let source_text = "import { answer } from 'pkg'; console.log(answer);\n";
        let target_text = "export const answer = 42;\n";
        let manifest_text = r#"{"exports":"./index.js"}"#;
        std::fs::write(&source, source_text).expect("write entry");
        std::fs::write(&target, target_text).expect("write package entry");
        std::fs::write(&manifest, manifest_text).expect("write package manifest");

        let root = fixture.canonicalize().expect("canonical root");
        let source = source.canonicalize().expect("canonical entry");
        let target = target.canonicalize().expect("canonical target");
        let manifest = manifest.canonicalize().expect("canonical manifest");
        let source_path = source.to_string_lossy().into_owned();
        let target_path = target.to_string_lossy().into_owned();
        let mut session = OneShot::start().expect("start an in-process engine");
        let plan = session
            .begin_sync(
                vec![
                    SourceHash {
                        path: source_path.clone(),
                        hash: source_digest(source_text),
                    },
                    SourceHash {
                        path: target_path.clone(),
                        hash: source_digest(target_text),
                    },
                ],
                ResolutionMap::new(
                    root.to_string_lossy(),
                    vec![PackageResolution {
                        importer: source_path.clone(),
                        specifier: "pkg".to_owned(),
                        target: target_path.clone(),
                        package_json: manifest.to_string_lossy().into_owned(),
                        package_json_hash: source_digest(manifest_text),
                    }],
                )
                .expect("valid resolution map"),
            )
            .expect("begin sync");
        for path in plan.paths {
            let text = if path == source_path {
                source_text
            } else if path == target_path {
                target_text
            } else {
                panic!("unexpected needed path {path}");
            };
            session
                .stage_source(plan.sync_id, path, text.to_owned())
                .expect("stage source");
        }
        let SyncCommit::Synced(view) = session.commit_sync(plan.sync_id).expect("commit sync")
        else {
            panic!("fresh sync became stale");
        };

        std::fs::remove_file(&manifest).expect("remove manifest after commit");
        let SourceResponse::Ready(summary) = session
            .run_at_view(source_path, root.to_string_lossy().into_owned(), view)
            .expect("run pinned graph")
        else {
            panic!("fresh source view became stale");
        };
        assert!(summary.succeeded, "{:?}", summary.diagnostics);
        assert_eq!(summary.stdout, "42\n");
        let _ = std::fs::remove_dir_all(&fixture);
    }
}

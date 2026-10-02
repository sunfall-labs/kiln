//! `--one-shot`: run the pipeline in this process and leave nothing behind.
//!
//! CI and reproducible builds must not depend on a background process that might be warm,
//! cold, or holding someone else's edits, so every command can be run against a database that
//! is created and destroyed by the invocation itself.
//!
//! `kiln-daemon::LocalClient` calls the same request handler as the socket server over a private
//! in-process channel. It keeps the handler's full-size stack, source-view transactions and peer
//! connections without requiring a Unix socket or leaving a directory behind.

use kiln_daemon::LocalClient;
use kiln_proto::{Request, Response};

use crate::backend::{Backend, interpret};
use crate::error::{Error, Result};

pub struct OneShot {
    client: LocalClient,
}

impl OneShot {
    pub fn start() -> Result<OneShot> {
        let client = LocalClient::new()
            .map_err(|source| Error::io("cannot start an in-process engine", source))?;
        Ok(OneShot { client })
    }
}

impl Backend for OneShot {
    fn peer(&self) -> Result<Box<dyn Backend>> {
        let peer = self
            .client
            .peer()
            .map_err(|source| Error::io("cannot open an in-process peer", source))?;
        Ok(Box::new(OneShot { client: peer }))
    }

    fn request(&mut self, request: Request) -> Result<Response> {
        let response = self
            .client
            .request(request)
            .map_err(|source| Error::io("cannot reach the in-process engine", source))?;
        interpret(response)
    }

    fn request_streaming(
        &mut self,
        request: Request,
        on_output: &mut dyn FnMut(kiln_proto::OutputStream, &str),
    ) -> Result<Response> {
        let response = self
            .client
            .request_streaming(request, on_output)
            .map_err(|source| Error::io("cannot reach the in-process engine", source))?;
        interpret(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{SourceResponse, SyncCommit};
    use crate::commands;
    use kiln_proto::{
        NodeModuleFormat, NodeRequestKind, NodeSourceFormat, PackageResolution, ResolutionMap,
        SourceHash, StepBudget, source_digest,
    };
    use std::path::PathBuf;

    // These tests exercise native behavior from fresh project roots. Their assertions cover
    // runtime behavior, not cache misses, so sharing content-addressed native artifacts avoids
    // recompiling the prelude for every temporary fixture. Tests that assert cache behavior keep
    // their private `.kiln/cache`; unsupported symlink setups fall back to that same private cache.
    #[cfg(unix)]
    fn share_native_semantic_cache(fixture: &std::path::Path) {
        let shared = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/kiln-cli-native-semantic-cache");
        let fixture_cache = fixture.join(".kiln/cache");
        if std::fs::create_dir_all(&shared).is_err()
            || std::fs::create_dir_all(fixture.join(".kiln")).is_err()
        {
            return;
        }
        let _ = std::os::unix::fs::symlink(shared, fixture_cache);
    }

    #[cfg(not(unix))]
    fn share_native_semantic_cache(_fixture: &std::path::Path) {}

    #[test]
    fn concurrent_one_shot_sessions_have_private_engines() {
        let mut first = OneShot::start().expect("start first engine");
        let mut second = OneShot::start().expect("start second engine");
        let Response::Stats(initial) = first.request(Request::Stats).expect("first stats") else {
            panic!("expected stats");
        };
        assert_eq!(initial.revision, 0);
        let Response::SourceUpdated { .. } = first
            .request(Request::SetSource {
                path: "a.js".into(),
                text: "export const a = 1;".into(),
                source_view: kiln_proto::SourceView(0),
            })
            .expect("set source")
        else {
            panic!("expected source update");
        };
        let Response::Stats(changed) = first.request(Request::Stats).expect("first stats") else {
            panic!("expected stats");
        };
        let Response::Stats(independent) = second.request(Request::Stats).expect("second stats")
        else {
            panic!("expected stats");
        };
        assert!(changed.revision > 0);
        assert_eq!(independent.revision, 0);

        let mut peer = first.peer().expect("open peer");
        let Response::Stats(shared) = peer.request(Request::Stats).expect("peer stats") else {
            panic!("expected stats");
        };
        assert_eq!(shared.revision, changed.revision);
    }

    #[test]
    fn a_one_shot_run_answers_and_then_leaves_nothing_behind() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-cli-1s-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create fixture");
        let source = fixture.join("a.js");
        std::fs::write(&source, "export const a = 1;\n").expect("write source");

        {
            let mut session = OneShot::start().expect("start an in-process engine");
            let out = commands::check(&mut session, &source, &fixture, false)
                .expect("check")
                .text;
            assert!(out.contains("a.js  r"), "{out}");
            // A fresh database has computed nothing before, so this check is all new work.
            assert!(out.contains("recomputed      parse 1"), "{out}");
        }
        let mut fresh = OneShot::start().expect("start a fresh engine");
        let Response::Stats(stats) = fresh.request(Request::Stats).expect("fresh stats") else {
            panic!("expected stats");
        };
        assert_eq!(stats.revision, 0);
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
            let output = commands::run(
                &mut session,
                &source,
                &[],
                &fixture,
                false,
                StepBudget::Unmetered,
            )
            .expect("run package graph");
            std::fs::write(package.join("index.js"), "export const answer = 43;\n")
                .expect("update package entry");
            let updated = commands::run(
                &mut session,
                &source,
                &[],
                &fixture,
                false,
                StepBudget::Unmetered,
            )
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
    fn one_shot_node_run_executes_a_builtin_module() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-cli-node-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create Node fixture");
        let source = fixture.join("main.mjs");
        std::fs::write(
            &source,
            "import { createServer } from 'node:http'; console.log(typeof createServer);\n",
        )
        .expect("write Node entry");

        let output = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::run(
                &mut session,
                &source,
                &[],
                &fixture,
                true,
                StepBudget::Unmetered,
            )
            .expect("run Node graph")
        };

        assert_eq!(output.text, "function\n");
        assert_eq!(output.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_native_node_run_uses_the_typed_builtin_graph() {
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-native-node-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create native Node fixture");
        let source = fixture.join("main.mjs");
        std::fs::write(
            &source,
            r#"import { createServer } from "node:http";
const server = createServer(() => {});
server.listen(0, "127.0.0.1", () => {
  console.log(server.address().port > 0);
  server.close();
});
"#,
        )
        .expect("write native Node entry");
        share_native_semantic_cache(&fixture);

        let output = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::native_run(
                &mut session,
                &source,
                &fixture,
                &[],
                kiln_proto::NativeProfile::Dev,
                true,
                false,
                None,
                None,
            )
            .expect("run native Node graph")
        };

        assert_eq!(output.text, "true\n");
        assert_eq!(output.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_native_node_runs_the_canonical_http_server_roundtrip() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-native-http-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create native HTTP fixture");
        share_native_semantic_cache(&fixture);
        let source = fixture.join("s01-http-hello.mjs");
        std::fs::copy(root.join("conformance/server/s01-http-hello.mjs"), &source)
            .expect("copy canonical native HTTP fixture");
        let output = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::native_run(
                &mut session,
                &source,
                &fixture,
                &[],
                kiln_proto::NativeProfile::Dev,
                true,
                false,
                None,
                None,
            )
            .expect("run canonical native Node server roundtrip")
        };

        assert_eq!(output.text, "200 text/plain kiln\n");
        assert_eq!(output.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_builds_a_standalone_node_server_artifact() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-native-node-build-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create native Node build fixture");
        share_native_semantic_cache(&fixture);
        let source = fixture.join("s01-http-hello.mjs");
        let artifact = fixture.join("s01-http-hello");
        std::fs::copy(root.join("conformance/server/s01-http-hello.mjs"), &source)
            .expect("copy canonical native HTTP fixture");
        let built = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::build(
                &mut session,
                &source,
                &fixture,
                Some(&artifact),
                kiln_proto::NativeProfile::Dev,
                false,
                true,
                false,
                false,
                None,
                false,
                None,
                None,
            )
            .expect("build standalone native Node server")
        };
        assert_eq!(built.exit_code, 0);
        assert!(artifact.is_file(), "{}", artifact.display());
        let output = std::process::Command::new(&artifact)
            .output()
            .expect("run standalone native Node server");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"200 text/plain kiln\n");
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_native_node_streams_request_bodies_through_hyper() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-native-http-stream-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create native HTTP stream fixture");
        share_native_semantic_cache(&fixture);
        let source = fixture.join("request-body-streams.mjs");
        std::fs::copy(
            root.join("conformance/server/s03-request-body-streams.mjs"),
            &source,
        )
        .expect("copy canonical native HTTP stream fixture");
        let output = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::native_run(
                &mut session,
                &source,
                &fixture,
                &[],
                kiln_proto::NativeProfile::Dev,
                true,
                false,
                None,
                None,
            )
            .expect("run canonical native Node request-body roundtrip")
        };

        assert_eq!(
            output.text, "200 {\"method\":\"POST\",\"bytes\":12,\"text\":\"kiln streams\"}\n",
            "exit={} notes={:?}",
            output.exit_code, output.notes
        );
        assert_eq!(output.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_builds_the_request_body_stream_server_as_a_standalone_artifact() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-native-http-stream-build-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create native HTTP stream build fixture");
        share_native_semantic_cache(&fixture);
        let source = fixture.join("request-body-streams.mjs");
        let artifact = fixture.join("request-body-streams");
        std::fs::copy(
            root.join("conformance/server/s03-request-body-streams.mjs"),
            &source,
        )
        .expect("copy canonical native HTTP stream fixture");
        let built = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::build(
                &mut session,
                &source,
                &fixture,
                Some(&artifact),
                kiln_proto::NativeProfile::Dev,
                false,
                true,
                false,
                false,
                None,
                false,
                None,
                None,
            )
            .expect("build standalone native request-body server")
        };
        assert_eq!(built.exit_code, 0);
        let output = std::process::Command::new(&artifact)
            .output()
            .expect("run standalone native request-body server");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            b"200 {\"method\":\"POST\",\"bytes\":12,\"text\":\"kiln streams\"}\n"
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_node_routes_two_http_responses_by_request_identity() {
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-http-request-identity-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create HTTP request identity fixture");
        let source = fixture.join("request-identities.mjs");
        std::fs::write(
            &source,
            r#"import http from "node:http";
const server = http.createServer((request, response) => {
  response.end(request.url);
});
server.listen(0, "127.0.0.1", () => {
  const results = [];
  const run = path => http.get({
    host: "127.0.0.1",
    port: server.address().port,
    path,
  }, response => {
    let body = "";
    response.setEncoding("utf8");
    response.on("data", chunk => body += chunk);
    response.on("end", () => {
      results.push(body);
      if (results.length === 1) run("/second");
      else {
        console.log(results.join(","));
        server.close();
      }
    });
  });
  run("/first");
});
"#,
        )
        .expect("write HTTP request identity fixture");
        share_native_semantic_cache(&fixture);

        let interpreted = {
            let mut session = OneShot::start().expect("start interpreted engine");
            commands::run(
                &mut session,
                &source,
                &[],
                &fixture,
                true,
                StepBudget::Unmetered,
            )
            .expect("run interpreted Node request identity fixture")
        };
        assert_eq!(interpreted.text, "/first,/second\n");
        assert_eq!(interpreted.exit_code, 0);

        let native = {
            let mut session = OneShot::start().expect("start native engine");
            commands::native_run(
                &mut session,
                &source,
                &fixture,
                &[],
                kiln_proto::NativeProfile::Dev,
                true,
                false,
                None,
                None,
            )
            .expect("run native Node request identity fixture")
        };
        assert_eq!(native.text, "/first,/second\n");
        assert_eq!(native.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_node_preserves_http_response_after_client_half_close() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-http-half-close-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create HTTP half-close fixture");
        let source = fixture.join("http-server-allow-half-open.mjs");
        std::fs::copy(
            root.join("conformance/node/upstream-adapted/http-server-allow-half-open.mjs"),
            &source,
        )
        .expect("copy pinned HTTP half-close fixture");
        share_native_semantic_cache(&fixture);

        let interpreted = {
            let mut session = OneShot::start().expect("start interpreted engine");
            commands::run(
                &mut session,
                &source,
                &[],
                &fixture,
                true,
                StepBudget::Unmetered,
            )
            .expect("run interpreted HTTP half-close fixture")
        };
        assert_eq!(interpreted.text, "pass\n");
        assert_eq!(interpreted.exit_code, 0);

        let native = {
            let mut session = OneShot::start().expect("start native engine");
            commands::native_run(
                &mut session,
                &source,
                &fixture,
                &[],
                kiln_proto::NativeProfile::Dev,
                true,
                false,
                None,
                None,
            )
            .expect("run native HTTP half-close fixture")
        };
        assert_eq!(
            native.text, "pass\n",
            "exit={} notes={:?} value={:?}",
            native.exit_code, native.notes, native.value
        );
        assert_eq!(native.exit_code, 0);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_node_run_selects_dual_package_exports_by_request_kind() {
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-dual-package-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        let package = fixture.join("node_modules/pkg");
        std::fs::create_dir_all(&package).expect("create dual package fixture");
        std::fs::write(
            package.join("package.json"),
            r#"{"exports":{"import":"./esm.mjs","require":"./cjs.cjs"}}"#,
        )
        .expect("write dual package manifest");
        std::fs::write(package.join("esm.mjs"), "export default 'import';\n")
            .expect("write import target");
        std::fs::write(package.join("cjs.cjs"), "module.exports = 'require';\n")
            .expect("write require target");
        std::fs::write(
            fixture.join("bridge.cjs"),
            "module.exports = require('pkg');\n",
        )
        .expect("write CommonJS bridge");
        let entry = fixture.join("main.mjs");
        std::fs::write(
            &entry,
            "import imported from 'pkg'; import required from './bridge.cjs'; console.log(imported, required);\n",
        )
        .expect("write dual package entry");

        let output = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::run(
                &mut session,
                &entry,
                &[],
                &fixture,
                true,
                StepBudget::Unmetered,
            )
            .expect("run dual package graph")
        };

        assert_eq!(output.text, "import require\n");
        assert_eq!(output.exit_code, 0);
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
        let source_text = "const { answer } = require('pkg'); console.log(answer);\n";
        let target_text = "module.exports = { answer: 42 };\n";
        let manifest_text = r#"{"exports":"./index.js","type":"commonjs"}"#;
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
                ResolutionMap::with_node_formats(
                    root.to_string_lossy(),
                    vec![PackageResolution {
                        importer: source_path.clone(),
                        specifier: "pkg".to_owned(),
                        target: target_path.clone(),
                        condition: NodeRequestKind::Require,
                        package_json: Some(manifest.to_string_lossy().into_owned()),
                        package_json_hash: Some(source_digest(manifest_text)),
                    }],
                    vec![
                        NodeSourceFormat {
                            path: source_path.clone(),
                            format: NodeModuleFormat::CommonJs,
                        },
                        NodeSourceFormat {
                            path: target_path.clone(),
                            format: NodeModuleFormat::CommonJs,
                        },
                    ],
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
            .run_at_view(
                source_path,
                root.to_string_lossy().into_owned(),
                Vec::new(),
                true,
                StepBudget::Unmetered,
                view,
                false,
            )
            .expect("run pinned graph")
        else {
            panic!("fresh source view became stale");
        };
        assert!(summary.succeeded, "{:?}", summary.diagnostics);
        assert_eq!(summary.stdout, "42\n");
        let _ = std::fs::remove_dir_all(&fixture);
    }

    /// ADR 0068 rule 4: `kiln build --no-kiln-extensions` fixes the answer into the binary, so it
    /// installs exactly Node's surface even when its environment asks for the extensions.
    #[test]
    fn one_shot_native_build_without_kiln_extensions_installs_node_surface() {
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-no-extensions-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create no-extensions fixture");
        let source = fixture.join("main.ts");
        let artifact = fixture.join("main");
        std::fs::write(
            &source,
            "const ta = Object.getPrototypeOf(Uint8Array.prototype);\n\
             console.log('parallelMap' in [], Object.getOwnPropertyNames(Array.prototype).includes('parallelMap'));\n\
             console.log('parallelMap' in ta, typeof (new Uint8Array(1) as any).parallelMap);\n\
             try { ([1, 2] as any).parallelMap((x: number) => x); console.log('called'); } catch (e) { console.log(e instanceof TypeError); }\n",
        )
        .expect("write source");
        share_native_semantic_cache(&fixture);

        let built = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::build(
                &mut session,
                &source,
                &fixture,
                Some(&artifact),
                kiln_proto::NativeProfile::Dev,
                false,
                false,
                false,
                true,
                None,
                false,
                None,
                None,
            )
            .expect("build without kiln extensions")
        };
        assert_eq!(built.exit_code, 0);
        let output = std::process::Command::new(&artifact)
            .env("KILN_EXTENSIONS", "1")
            .output()
            .expect("run the binary built without kiln extensions");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "false false\nfalse undefined\ntrue\n"
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn one_shot_native_build_with_explain_prints_facts() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-cli-explain-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create explain fixture");
        let source = fixture.join("main.ts");
        let artifact = fixture.join("main");
        std::fs::write(
            &source,
            "export const greeting = 'hello';\nexport function add(a: number, b: number): number { return a + b; }\nconsole.log(add(1, 2));\n",
        )
        .expect("write source");
        share_native_semantic_cache(&fixture);

        let built = {
            let mut session = OneShot::start().expect("start an in-process engine");
            commands::build(
                &mut session,
                &source,
                &fixture,
                Some(&artifact),
                kiln_proto::NativeProfile::Dev,
                false,
                false,
                true,
                false,
                None,
                false,
                None,
                None,
            )
            .expect("build with explain")
        };

        assert_eq!(built.exit_code, 0);
        assert!(
            built.text.contains("Whole-Program Fixpoint Facts"),
            "{}",
            built.text
        );
        assert!(built.text.contains("World: closed"), "{}", built.text);
        assert!(built.text.contains("Immutable Bindings:"), "{}", built.text);
        assert!(built.text.contains("greeting"), "{}", built.text);
        assert!(built.text.contains("Function add"), "{}", built.text);
        assert!(built.text.contains("Sites:"), "{}", built.text);
        let _ = std::fs::remove_dir_all(&fixture);
    }

    /// A server's output has to reach the client while the server runs. `kiln run` used to hold
    /// every line until the program exited, which for a server is never: `listen`'s callback ran
    /// and printed "listening", and nothing was shown until the process was killed.
    ///
    /// The server closes itself only after it has answered a request, and the request is sent
    /// from the client's output callback the moment "listening" arrives. A client that is only
    /// shown output at exit therefore never sends it; the watchdog sends it after a delay, so
    /// that failure is an assertion rather than a hung test.
    #[test]
    fn a_servers_listening_line_reaches_the_client_before_the_server_exits() {
        use std::io::{Read, Write};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Spy {
            inner: OneShot,
            seen: Vec<(kiln_proto::OutputStream, String)>,
            request_sent_by: Arc<std::sync::Mutex<Option<&'static str>>>,
            sent: Arc<AtomicBool>,
            port: u16,
        }

        fn get(port: u16) {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream
                .write_all(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
                .expect("send request");
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
        }

        impl Backend for Spy {
            fn request(&mut self, request: Request) -> Result<Response> {
                self.inner.request(request)
            }

            fn request_streaming(
                &mut self,
                request: Request,
                _on_output: &mut dyn FnMut(kiln_proto::OutputStream, &str),
            ) -> Result<Response> {
                let Spy {
                    inner,
                    seen,
                    request_sent_by,
                    sent,
                    port,
                } = self;
                inner.request_streaming(request, &mut |stream, text| {
                    seen.push((stream, text.to_owned()));
                    if text.contains("listening") && !sent.swap(true, Ordering::SeqCst) {
                        *request_sent_by.lock().unwrap() = Some("output");
                        get(*port);
                    }
                })
            }
        }

        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("find a free port")
            .local_addr()
            .expect("local address")
            .port();
        let fixture = std::env::temp_dir().join(format!(
            "kiln-cli-streamed-listen-fixture-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&fixture);
        std::fs::create_dir_all(&fixture).expect("create streamed listen fixture");
        let source = fixture.join("server.mjs");
        std::fs::write(
            &source,
            format!(
                "import http from 'node:http';\n\
                 const server = http.createServer((req, res) => {{\n\
                   console.log('request');\n\
                   res.end('ok');\n\
                   server.close();\n\
                 }});\n\
                 server.listen({port}, '127.0.0.1', () => console.log('listening'));\n"
            ),
        )
        .expect("write server");

        let sent = Arc::new(AtomicBool::new(false));
        let request_sent_by = Arc::new(std::sync::Mutex::new(None));
        {
            let sent = Arc::clone(&sent);
            let by = Arc::clone(&request_sent_by);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(30));
                if !sent.swap(true, Ordering::SeqCst) {
                    *by.lock().unwrap() = Some("watchdog");
                    get(port);
                }
            });
        }
        let mut spy = Spy {
            inner: OneShot::start().expect("start an in-process engine"),
            seen: Vec::new(),
            request_sent_by: Arc::clone(&request_sent_by),
            sent,
            port,
        };
        let output = commands::run(
            &mut spy,
            &source,
            &[],
            &fixture,
            true,
            StepBudget::Unmetered,
        )
        .expect("run the server");

        assert_eq!(output.exit_code, 0);
        assert_eq!(
            *request_sent_by.lock().unwrap(),
            Some("output"),
            "the client was not shown 'listening' while the server was still running"
        );
        let streamed: Vec<&str> = spy.seen.iter().map(|(_, text)| text.as_str()).collect();
        assert_eq!(streamed.concat(), "listening\nrequest\n", "{streamed:?}");
        assert_eq!(
            output.text, "",
            "streamed output is not repeated in the summary"
        );
        let _ = std::fs::remove_dir_all(&fixture);
    }
}

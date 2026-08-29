//! Turning one [`Request`] into one [`Response`] against the query engine.
//!
//! Deliberately free of I/O on the socket and of any knowledge of threads: everything here runs
//! while the engine lock is held, so it must be short, and it must be testable without a server.

use kiln_db::QueryEngine;
use kiln_proto::{
    Diagnostic, FileId, ModuleSummary, NativeBuildSummary, NativeProfile, NativeRunSummary,
    PROTOCOL_VERSION, ProjectSummary, Request, Response, RunSummary, Stats,
};
use std::path::{Path, PathBuf};

/// What the connection loop should do once the response has been written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Keep reading requests from this client.
    Continue,
    /// Answer, then hang up on this client. The daemon keeps serving everyone else.
    CloseConnection,
    /// Answer, hang up, and stop the daemon.
    StopDaemon,
}

pub fn dispatch<E: QueryEngine>(engine: &mut E, request: Request) -> (Response, Next) {
    match request {
        Request::Ping { protocol } if protocol != PROTOCOL_VERSION => (
            Response::Error {
                message: format!(
                    "protocol mismatch: client speaks version {protocol}, this daemon speaks \
                     {PROTOCOL_VERSION}; restart the daemon so it matches the client"
                ),
            },
            // Nothing this client goes on to send can be trusted to mean what we think it
            // means, so refuse the whole connection rather than only this request.
            Next::CloseConnection,
        ),
        Request::Ping { .. } => (
            Response::Pong {
                protocol: PROTOCOL_VERSION,
                pid: std::process::id(),
            },
            Next::Continue,
        ),

        Request::Sync { files } => {
            // The daemon holds the authoritative copy, so it decides what it is missing rather
            // than the client guessing. Hashing what we hold on demand costs microseconds and
            // avoids a second cache that could disagree with the engine.
            //
            // Note what this does NOT do: a file the daemon holds and the client omits is left
            // alone. Dropping it would need a removal on QueryEngine, which does not exist yet;
            // the pre-existing behaviour was the same, so nothing regresses here.
            let mut needed = Vec::new();
            for file in files {
                let id = engine.intern(&file.path);
                let held = engine.source(id).map(kiln_db::content_hash);
                if held != Some(file.hash) {
                    needed.push(file.path);
                }
            }
            (Response::Needed { paths: needed }, Next::Continue)
        }

        Request::SetSource { path, text } => {
            let file = engine.intern(&path);
            engine.set_source(file, text);
            (Response::Ok, Next::Continue)
        }

        Request::Check { path } => (check(engine, path), Next::Continue),
        Request::Run { path } => (run(engine, path), Next::Continue),
        Request::NativeBuild {
            path,
            project_root,
            output,
            profile,
        } => (
            match native_program(engine, &path) {
                Ok(program) => native_build_program(program, path, project_root, output, profile),
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
        Request::NativeRun {
            path,
            project_root,
            profile,
        } => (
            match native_program(engine, &path) {
                Ok(program) => native_run_program(program, path, project_root, profile),
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
        Request::Test {
            root,
            mode,
            changed_paths,
            previously_failed,
        } => (
            match crate::testing::run(&root, mode, changed_paths, previously_failed) {
                Ok(summary) => Response::Tested(summary),
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),

        Request::Project => {
            let project = engine.project();
            (
                Response::Project(ProjectSummary {
                    modules: project.modules,
                    exports: project.exports,
                    interface_hash: project.interface_hash,
                }),
                Next::Continue,
            )
        }

        Request::Stats => (Response::Stats(stats(engine)), Next::Continue),

        Request::ResetStats => {
            engine.reset_stats();
            (Response::Ok, Next::Continue)
        }

        Request::Shutdown => (Response::Ok, Next::StopDaemon),
    }
}

/// Run the pipeline for one module.
///
/// Reading from disk only when the file has no source yet is what makes an editor-driven
/// `SetSource` authoritative: once a client has pushed unsaved buffer contents, later `Check`s
/// must see the buffer, not the stale bytes on disk.
fn check<E: QueryEngine>(engine: &mut E, path: String) -> Response {
    let file = match ensure_source(engine, &path) {
        Ok(file) => file,
        Err(message) => return Response::Error { message },
    };

    let parsed = engine.parse(file);
    // The interface, not `parsed.exports`, is the externally visible surface, and it is what a
    // dependent would be recompiled against - so it is what the summary reports.
    let interface = engine.interface(file);

    Response::Checked(ModuleSummary {
        path,
        file: FileId(file.0),
        implementation_hash: parsed.implementation_hash,
        interface_hash: interface.interface_hash,
        exports: interface.exports,
        diagnostics: parsed
            .module
            .diagnostics
            .into_iter()
            .map(|diagnostic| Diagnostic {
                message: diagnostic.message,
                start: diagnostic.span.map(|span| span.start),
                end: diagnostic.span.map(|span| span.end),
            })
            .collect(),
    })
}

fn run<E: QueryEngine>(engine: &mut E, path: String) -> Response {
    let file = match ensure_source(engine, &path) {
        Ok(file) => file,
        Err(message) => return Response::Error { message },
    };
    let program = match engine.ir(file) {
        Ok(program) => program,
        Err(diagnostics) => {
            return Response::Ran(RunSummary {
                path,
                file: FileId(file.0),
                succeeded: false,
                stdout: String::new(),
                value: None,
                instructions: 0,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|diagnostic| Diagnostic {
                        message: diagnostic.message,
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
        }
    };

    let mut host = kiln_runtime::CaptureHost::new();
    match kiln_interpreter::execute(&program, &mut host, Default::default()) {
        Ok(execution) => Response::Ran(RunSummary {
            path,
            file: FileId(file.0),
            succeeded: true,
            stdout: host.into_stdout(),
            value: Some(execution.value.to_display_string()),
            instructions: execution.steps,
            diagnostics: Vec::new(),
        }),
        Err(error) => Response::Ran(RunSummary {
            path,
            file: FileId(file.0),
            succeeded: false,
            stdout: host.into_stdout(),
            value: None,
            instructions: 0,
            diagnostics: vec![Diagnostic {
                message: error.to_string(),
                start: None,
                end: None,
            }],
        }),
    }
}

pub(crate) fn native_build_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = output.map_or_else(
        || default_native_output(&project_root, &path),
        PathBuf::from,
    );
    if output == Path::new(&path)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&path))
    {
        return Response::Error {
            message: format!("native output would overwrite source file {path}"),
        };
    }
    let builder =
        match kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache")) {
            Ok(builder) => builder,
            Err(error) => {
                return Response::Error {
                    message: error.to_string(),
                };
            }
        };
    match builder.build(
        &program,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => Response::NativeBuilt(native_summary(report)),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_run_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = default_native_output(&project_root, &path);
    let builder =
        match kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache")) {
            Ok(builder) => builder,
            Err(error) => {
                return Response::Error {
                    message: error.to_string(),
                };
            }
        };
    match builder.run(
        &program,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => Response::NativeRan(NativeRunSummary {
            exit_code: report.exit_code,
            stdout: String::from_utf8_lossy(&report.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&report.stderr).into_owned(),
            build: native_summary(report.build),
        }),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_program<E: QueryEngine>(
    engine: &mut E,
    path: &str,
) -> Result<kiln_ir::Program, String> {
    let file = ensure_source(engine, path)?;
    engine.ir(file).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| match diagnostic.span {
                Some(span) => format!(
                    "{path}:{}..{}: {}",
                    span.start, span.end, diagnostic.message
                ),
                None => format!("{path}: {}", diagnostic.message),
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn native_profile(profile: NativeProfile) -> kiln_native::NativeProfile {
    match profile {
        NativeProfile::Dev => kiln_native::NativeProfile::Dev,
        NativeProfile::Check => kiln_native::NativeProfile::Check,
        NativeProfile::Release => kiln_native::NativeProfile::Release,
    }
}

fn default_native_output(project_root: &Path, source: &str) -> PathBuf {
    let stem = Path::new(source)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let mut output = project_root.join(".kiln/bin").join(stem);
    if cfg!(windows) {
        output.set_extension("exe");
    }
    output
}

fn native_summary(report: kiln_native::NativeBuildReport) -> NativeBuildSummary {
    NativeBuildSummary {
        module_symbol: report.module_symbol,
        output_path: report.output.to_string_lossy().into_owned(),
        compiler_identity: format!(
            "{} {} targeting {} ({})",
            report.tool.kind,
            report.tool.version,
            report.tool.target,
            report.tool.path.display()
        ),
        generated_c_cache_hit: report.cache_hits.generated_c,
        app_object_cache_hit: report.cache_hits.application_object,
        runtime_object_cache_hit: report.cache_hits.runtime_object,
        launcher_object_cache_hit: report.cache_hits.launcher_object,
        executable_cache_hit: report.cache_hits.executable,
        generated_c_bytes: report.sizes.generated_c,
        app_object_bytes: report.sizes.application_object,
        runtime_object_bytes: report.sizes.runtime_object,
        launcher_object_bytes: report.sizes.launcher_object,
        executable_bytes: report.sizes.executable,
        emit_millis: millis(report.timings.emit),
        compile_millis: millis(report.timings.compile),
        link_millis: millis(report.timings.link),
        total_millis: millis(report.timings.total),
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn ensure_source<E: QueryEngine>(engine: &mut E, path: &str) -> Result<kiln_db::FileId, String> {
    let file = engine.intern(path);
    if engine.source(file).is_none() {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        engine.set_source(file, text);
    }
    Ok(file)
}

fn stats<E: QueryEngine>(engine: &E) -> Stats {
    let stats = engine.stats();
    Stats {
        revision: stats.revision,
        parse_computed: stats.parse_computed,
        ir_computed: stats.ir_computed,
        interface_computed: stats.interface_computed,
        project_computed: stats.project_computed,
        validated: stats.validated,
        early_cutoff: stats.early_cutoff,
    }
}

//! Turning one [`Request`] into one [`Response`] against the query engine.
//!
//! Deliberately free of I/O on the socket and of any knowledge of threads: everything here runs
//! while the engine lock is held, so it must be short, and it must be testable without a server.

use kiln_db::QueryEngine;
use kiln_proto::{
    FileId, ModuleSummary, PROTOCOL_VERSION, ProjectSummary, Request, Response, Stats,
};

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

        Request::SetSource { path, text } => {
            let file = engine.intern(&path);
            engine.set_source(file, text);
            (Response::Ok, Next::Continue)
        }

        Request::Check { path } => (check(engine, path), Next::Continue),

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
    let file = engine.intern(&path);
    if engine.source(file).is_none() {
        match std::fs::read_to_string(&path) {
            Ok(text) => engine.set_source(file, text),
            Err(error) => {
                return Response::Error {
                    message: format!("cannot read {path}: {error}"),
                };
            }
        }
    }

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
    })
}

fn stats<E: QueryEngine>(engine: &E) -> Stats {
    let stats = engine.stats();
    Stats {
        revision: stats.revision,
        parse_computed: stats.parse_computed,
        interface_computed: stats.interface_computed,
        project_computed: stats.project_computed,
        validated: stats.validated,
        early_cutoff: stats.early_cutoff,
    }
}

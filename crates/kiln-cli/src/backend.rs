//! What the commands talk to.
//!
//! Two things can answer a [`Request`]: a daemon over a socket, or an in-process engine when
//! `--one-shot` is used. The commands are written against this trait so neither mode is the
//! special case, and so the tests can drive them without either.

use kiln_proto::{
    ModuleSummary, NativeBuildSummary, NativeProfile, NativeRunSummary, ProjectSummary, Request,
    Response, RunSummary, SourceHash, Stats, TestMode, TestSummary,
};

use crate::error::{Error, Result};

pub trait Backend {
    /// Send one request and return its response. Implementations must already have turned
    /// [`Response::Error`] into [`Error::Daemon`] via [`interpret`].
    fn request(&mut self, request: Request) -> Result<Response>;

    fn set_source(&mut self, path: String, text: String) -> Result<()> {
        match self.request(Request::SetSource { path, text })? {
            Response::Ok => Ok(()),
            other => Err(mismatch("SetSource", &other)),
        }
    }

    /// Name every file, and learn which ones actually have to be sent.
    fn sync(&mut self, files: Vec<SourceHash>) -> Result<Vec<String>> {
        match self.request(Request::Sync { files })? {
            Response::Needed { paths } => Ok(paths),
            other => Err(mismatch("Sync", &other)),
        }
    }

    fn check(&mut self, path: String) -> Result<ModuleSummary> {
        match self.request(Request::Check { path })? {
            Response::Checked(summary) => Ok(summary),
            other => Err(mismatch("Check", &other)),
        }
    }

    fn run(&mut self, path: String, project_root: String) -> Result<RunSummary> {
        match self.request(Request::Run { path, project_root })? {
            Response::Ran(summary) => Ok(summary),
            other => Err(mismatch("Run", &other)),
        }
    }

    fn native_build(
        &mut self,
        path: String,
        project_root: String,
        output: Option<String>,
        profile: NativeProfile,
    ) -> Result<NativeBuildSummary> {
        match self.request(Request::NativeBuild {
            path,
            project_root,
            output,
            profile,
        })? {
            Response::NativeBuilt(summary) => Ok(summary),
            other => Err(mismatch("NativeBuild", &other)),
        }
    }

    fn native_run(
        &mut self,
        path: String,
        project_root: String,
        profile: NativeProfile,
    ) -> Result<NativeRunSummary> {
        match self.request(Request::NativeRun {
            path,
            project_root,
            profile,
        })? {
            Response::NativeRan(summary) => Ok(summary),
            other => Err(mismatch("NativeRun", &other)),
        }
    }

    fn test(
        &mut self,
        root: String,
        mode: TestMode,
        changed_paths: Option<Vec<String>>,
        previously_failed: Vec<String>,
    ) -> Result<TestSummary> {
        match self.request(Request::Test {
            root,
            mode,
            changed_paths,
            previously_failed,
        })? {
            Response::Tested(summary) => Ok(summary),
            other => Err(mismatch("Test", &other)),
        }
    }

    fn project(&mut self) -> Result<ProjectSummary> {
        match self.request(Request::Project)? {
            Response::Project(summary) => Ok(summary),
            other => Err(mismatch("Project", &other)),
        }
    }

    fn stats(&mut self) -> Result<Stats> {
        match self.request(Request::Stats)? {
            Response::Stats(stats) => Ok(stats),
            other => Err(mismatch("Stats", &other)),
        }
    }
}

/// Lift a daemon-reported failure out of the response type. Every backend funnels through
/// this so that `Response::Error` can never be mistaken for a wrong-variant bug.
pub fn interpret(response: Response) -> Result<Response> {
    match response {
        Response::Error { message } => Err(Error::Daemon(message)),
        other => Ok(other),
    }
}

fn mismatch(request: &str, response: &Response) -> Error {
    Error::Protocol(format!("{request} was answered with {}", name_of(response)))
}

fn name_of(response: &Response) -> &'static str {
    match response {
        Response::Pong { .. } => "Pong",
        Response::Checked(_) => "Checked",
        Response::Ran(_) => "Ran",
        Response::NativeBuilt(_) => "NativeBuilt",
        Response::NativeRan(_) => "NativeRan",
        Response::Tested(_) => "Tested",
        Response::Project(_) => "Project",
        Response::Stats(_) => "Stats",
        Response::Needed { .. } => "Needed",
        Response::Ok => "Ok",
        Response::Error { .. } => "Error",
    }
}

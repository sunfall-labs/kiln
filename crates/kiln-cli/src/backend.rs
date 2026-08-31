//! What the commands talk to.
//!
//! Two things can answer a [`Request`]: a daemon over a socket, or an in-process engine when
//! `--one-shot` is used. The commands are written against this trait so neither mode is the
//! special case, and so the tests can drive them without either.

use kiln_proto::{
    CacheGcSummary, ModuleSummary, NativeBuildSummary, NativeProfile, NativeRunSummary,
    NativeSharedLibrarySummary, ProjectSummary, Request, ResolutionMap, Response, RunSummary,
    SourceHash, SourceView, Stats, SyncId, TestMode, TestSummary,
};

use crate::error::{Error, Result};

pub trait Backend: Send {
    /// Send one request and return its response. Implementations must already have turned
    /// [`Response::Error`] into [`Error::Daemon`] via [`interpret`].
    fn request(&mut self, request: Request) -> Result<Response>;

    /// Open an independent request/response channel to the same engine. Watch uses this to
    /// keep a slow native toolchain off the interpreter's critical path.
    fn peer(&self) -> Result<Box<dyn Backend>> {
        Err(Error::Protocol(
            "backend does not support peer connections".into(),
        ))
    }

    fn native_build_at_revision_at_view(
        &mut self,
        path: String,
        project_root: String,
        output: Option<String>,
        profile: NativeProfile,
        source_view: SourceView,
        expected_revision: u64,
    ) -> Result<NativeBuildAtRevision> {
        match self.request(Request::NativeBuildAtRevision {
            path,
            project_root,
            output,
            profile,
            source_view,
            expected_revision,
        })? {
            Response::NativeBuilt(summary) => Ok(NativeBuildAtRevision::Built(summary)),
            Response::Stale { current_revision } => {
                Ok(NativeBuildAtRevision::Stale { current_revision })
            }
            Response::StaleSourceView { current } => {
                Ok(NativeBuildAtRevision::StaleSourceView { current })
            }
            other => Err(mismatch("NativeBuildAtRevision", &other)),
        }
    }

    fn set_source_at_view(
        &mut self,
        path: String,
        text: String,
        source_view: SourceView,
    ) -> Result<SourceUpdate> {
        match self.request(Request::SetSource {
            path,
            text,
            source_view,
        })? {
            Response::SourceUpdated { source_view } => Ok(SourceUpdate::Updated(source_view)),
            Response::StaleSourceView { current } => Ok(SourceUpdate::Stale(current)),
            other => Err(mismatch("SetSource", &other)),
        }
    }

    /// Name every file, and learn which ones actually have to be sent.
    fn begin_sync(
        &mut self,
        files: Vec<SourceHash>,
        resolutions: ResolutionMap,
    ) -> Result<SyncPlan> {
        match self.request(Request::Sync { files, resolutions })? {
            Response::Needed { sync_id, paths } => Ok(SyncPlan { sync_id, paths }),
            other => Err(mismatch("Sync", &other)),
        }
    }

    fn stage_source(&mut self, sync_id: SyncId, path: String, text: String) -> Result<()> {
        match self.request(Request::StageSource {
            sync_id,
            path,
            text,
        })? {
            Response::Ok => Ok(()),
            other => Err(mismatch("StageSource", &other)),
        }
    }

    fn commit_sync(&mut self, sync_id: SyncId) -> Result<SyncCommit> {
        match self.request(Request::CommitSync { sync_id })? {
            Response::Synced { source_view } => Ok(SyncCommit::Synced(source_view)),
            Response::StaleSourceView { current } => Ok(SyncCommit::Stale(current)),
            other => Err(mismatch("CommitSync", &other)),
        }
    }

    fn check_at_view(
        &mut self,
        path: String,
        project_root: String,
        source_view: SourceView,
    ) -> Result<SourceResponse<ModuleSummary>> {
        match self.request(Request::Check {
            path,
            project_root,
            source_view,
        })? {
            Response::Checked(summary) => Ok(SourceResponse::Ready(summary)),
            Response::StaleSourceView { current } => Ok(SourceResponse::Stale(current)),
            other => Err(mismatch("Check", &other)),
        }
    }
    fn run_at_view(
        &mut self,
        path: String,
        project_root: String,
        node: bool,
        source_view: SourceView,
    ) -> Result<SourceResponse<RunSummary>> {
        match self.request(Request::Run {
            path,
            project_root,
            node,
            source_view,
        })? {
            Response::Ran(summary) => Ok(SourceResponse::Ready(summary)),
            Response::StaleSourceView { current } => Ok(SourceResponse::Stale(current)),
            other => Err(mismatch("Run", &other)),
        }
    }
    fn native_build_at_view(
        &mut self,
        path: String,
        project_root: String,
        output: Option<String>,
        profile: NativeProfile,
        source_view: SourceView,
    ) -> Result<SourceResponse<NativeBuildSummary>> {
        match self.request(Request::NativeBuild {
            path,
            project_root,
            output,
            profile,
            source_view,
        })? {
            Response::NativeBuilt(summary) => Ok(SourceResponse::Ready(summary)),
            Response::StaleSourceView { current } => Ok(SourceResponse::Stale(current)),
            other => Err(mismatch("NativeBuild", &other)),
        }
    }
    fn native_run_at_view(
        &mut self,
        path: String,
        project_root: String,
        profile: NativeProfile,
        node: bool,
        source_view: SourceView,
    ) -> Result<SourceResponse<NativeRunSummary>> {
        match self.request(Request::NativeRun {
            path,
            project_root,
            profile,
            node,
            source_view,
        })? {
            Response::NativeRan(summary) => Ok(SourceResponse::Ready(summary)),
            Response::StaleSourceView { current } => Ok(SourceResponse::Stale(current)),
            other => Err(mismatch("NativeRun", &other)),
        }
    }
    fn native_shared_build_at_view(
        &mut self,
        path: String,
        project_root: String,
        output: Option<String>,
        profile: NativeProfile,
        source_view: SourceView,
    ) -> Result<SourceResponse<NativeSharedLibrarySummary>> {
        match self.request(Request::NativeSharedBuild {
            path,
            project_root,
            output,
            profile,
            source_view,
        })? {
            Response::NativeSharedBuilt(summary) => Ok(SourceResponse::Ready(summary)),
            Response::StaleSourceView { current } => Ok(SourceResponse::Stale(current)),
            other => Err(mismatch("NativeSharedBuild", &other)),
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

    fn project_at_view(
        &mut self,
        source_view: SourceView,
    ) -> Result<SourceResponse<ProjectSummary>> {
        match self.request(Request::Project { source_view })? {
            Response::Project(summary) => Ok(SourceResponse::Ready(summary)),
            Response::StaleSourceView { current } => Ok(SourceResponse::Stale(current)),
            other => Err(mismatch("Project", &other)),
        }
    }

    /// Sweep the project's artifact cache. Carries no source view: cache records are addressed
    /// by content, so no edit can make this request stale.
    fn cache_gc(&mut self, project_root: String, dry_run: bool) -> Result<CacheGcSummary> {
        match self.request(Request::CacheGc {
            project_root,
            dry_run,
        })? {
            Response::CacheSwept(summary) => Ok(summary),
            other => Err(mismatch("CacheGc", &other)),
        }
    }

    fn stats(&mut self) -> Result<Stats> {
        match self.request(Request::Stats)? {
            Response::Stats(stats) => Ok(stats),
            other => Err(mismatch("Stats", &other)),
        }
    }
}

#[derive(Debug)]
pub enum NativeBuildAtRevision {
    Built(NativeBuildSummary),
    Stale { current_revision: u64 },
    StaleSourceView { current: SourceView },
}

#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub sync_id: SyncId,
    pub paths: Vec<String>,
}
#[derive(Debug, Clone, Copy)]
pub enum SyncCommit {
    Synced(SourceView),
    Stale(SourceView),
}
#[derive(Debug, Clone, Copy)]
pub enum SourceUpdate {
    Updated(SourceView),
    Stale(SourceView),
}
#[derive(Debug, Clone)]
pub enum SourceResponse<T> {
    Ready(T),
    Stale(SourceView),
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
        Response::Stale { .. } => "Stale",
        Response::NativeSharedBuilt(_) => "NativeSharedBuilt",
        Response::NativeRan(_) => "NativeRan",
        Response::Tested(_) => "Tested",
        Response::Project(_) => "Project",
        Response::Stats(_) => "Stats",
        Response::CacheSwept(_) => "CacheSwept",
        Response::Needed { .. } => "Needed",
        Response::Synced { .. } => "Synced",
        Response::SourceUpdated { .. } => "SourceUpdated",
        Response::StaleSourceView { .. } => "StaleSourceView",
        Response::Ok => "Ok",
        Response::Error { .. } => "Error",
    }
}

//! What the commands talk to.
//!
//! Two things can answer a [`Request`]: a daemon over a socket, or an in-process engine when
//! `--one-shot` is used. The commands are written against this trait so neither mode is the
//! special case, and so the tests can drive them without either.

use kiln_proto::{ModuleSummary, ProjectSummary, Request, Response, Stats};

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

    fn check(&mut self, path: String) -> Result<ModuleSummary> {
        match self.request(Request::Check { path })? {
            Response::Checked(summary) => Ok(summary),
            other => Err(mismatch("Check", &other)),
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
        Response::Project(_) => "Project",
        Response::Stats(_) => "Stats",
        Response::Ok => "Ok",
        Response::Error { .. } => "Error",
    }
}

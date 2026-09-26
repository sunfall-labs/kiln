//! One error type for the whole client.
//!
//! The CLI is a thin shell around the daemon, so the interesting distinction is not the
//! source of a failure but what a user should do about it: fix their command line (exit 2),
//! or deal with a broken daemon / bad file (exit 1).

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// The command line was wrong. Callers get usage help, not a stack of context.
    Usage(String),
    /// A path the user named does not exist. Separate from [`Error::Usage`] because `watch`
    /// has to tell a mid-save rename apart from a genuinely bad argument.
    MissingFile(String),
    /// The command line was fine but the file it named is not usable. Exits like a usage
    /// error, but suggesting `--help` for a file with the wrong encoding would be unhelpful.
    BadInput(String),
    /// An I/O failure, labelled with what was being attempted.
    Io { what: String, source: io::Error },
    /// The daemon answered, but not with something we can use.
    Protocol(String),
    /// The daemon answered with `Response::Error`.
    Daemon(String),
    /// No daemon is listening and we were not asked to start one.
    NotRunning,
}

impl Error {
    pub fn io(what: impl Into<String>, source: io::Error) -> Self {
        Error::Io {
            what: what.into(),
            source,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Error::Usage(message.into())
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) | Error::MissingFile(_) | Error::BadInput(_) => 2,
            _ => 1,
        }
    }

    /// A closed stdout (`kiln stats | head -1`) is a normal way for a run to end, not a
    /// failure, so main exits quietly instead of printing a broken-pipe error.
    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, Error::Io { source, .. } if source.kind() == io::ErrorKind::BrokenPipe)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(message) => write!(f, "{message}"),
            Error::MissingFile(path) => write!(f, "no such file: {path}"),
            Error::BadInput(message) => write!(f, "{message}"),
            Error::Io { what, source } => write!(f, "{what}: {source}"),
            Error::Protocol(message) => write!(f, "protocol error: {message}"),
            Error::Daemon(message) => write!(f, "{message}"),
            Error::NotRunning => write!(f, "no daemon is running"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

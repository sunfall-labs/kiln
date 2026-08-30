//! `kiln` - the command line client.
//!
//! The client is deliberately thin. Everything expensive lives in the daemon, so a command is
//! a socket round trip rather than a process start (design.md §1), and the CLI's job is to
//! decide which backend answers, sequence a few requests, and print the result.
//!
//! What it prints is part of the design, not decoration. Every query command reports what the
//! engine had to recompute and what it reused, because that is the only way a developer can
//! see that incrementality is working - or notice the day it stops.

mod args;
mod backend;
mod client;
mod commands;
mod error;
mod lifecycle;
mod oneshot;
mod render;
mod sources;
mod watch;

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use args::{CacheAction, Command, DaemonAction, Invocation, Parsed};
use backend::Backend;
use client::DaemonClient;
use commands::Output;
use error::{Error, Result};
use oneshot::OneShot;

fn main() {
    let code = match run() {
        Ok(code) => code,
        // Downstream of a `| head`, a closed pipe is how a run normally ends.
        Err(error) if error.is_broken_pipe() => 0,
        Err(error) => {
            eprintln!("kiln: {error}");
            if matches!(error, Error::Usage(_)) {
                eprintln!("kiln: run `kiln --help` for usage");
            }
            error.exit_code()
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    match args::parse(utf8_args(std::env::args_os().skip(1))?)? {
        Parsed::Help => {
            emit(args::HELP)?;
            Ok(0)
        }
        Parsed::Version => {
            emit(&version())?;
            Ok(0)
        }
        Parsed::Run(invocation) => dispatch(invocation),
    }
}

/// `std::env::args` panics on a non-UTF-8 argument, and an argument is entitled to be one:
/// on Unix it is a byte string, and so is a path. Rejecting it here rather than carrying the
/// bytes further is the honest answer for a path as well as for a flag - `Request::Check`
/// carries a `String`, and `commands::canonical_path` already refuses a path it cannot
/// decode - so accepting the argument would only move the same failure downstream.
///
/// Taking the arguments as a parameter rather than reading the environment is what makes the
/// non-UTF-8 case testable at all.
fn utf8_args<I>(args: I) -> Result<Vec<String>>
where
    I: IntoIterator<Item = OsString>,
{
    args.into_iter()
        .map(|arg| {
            arg.into_string().map_err(|bad| {
                Error::usage(format!(
                    "argument is not valid UTF-8: {}",
                    bad.to_string_lossy()
                ))
            })
        })
        .collect()
}

fn version() -> String {
    format!(
        "kiln {}\nprotocol {}\n",
        env!("CARGO_PKG_VERSION"),
        kiln_proto::PROTOCOL_VERSION
    )
}

fn dispatch(invocation: Invocation) -> Result<i32> {
    // The working directory is the project: it decides which daemon answers and what
    // `kiln project` covers. A configured project root is a later change.
    let root = std::env::current_dir()
        .map_err(|source| Error::io("cannot read the working directory", source))?;
    let socket = client::socket_path(invocation.socket, &root)?;
    let one_shot = invocation.one_shot;

    match invocation.command {
        Command::Run {
            path,
            native: false,
            ..
        } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::run(backend.as_mut(), &path, &root)?)
        }
        Command::Run {
            path,
            native: true,
            profile,
        } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::native_run(
                backend.as_mut(),
                &path,
                &root,
                profile,
            )?)
        }
        Command::Build {
            path,
            output,
            profile,
            shared,
        } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::build(
                backend.as_mut(),
                &path,
                &root,
                output.as_deref(),
                profile,
                shared,
            )?)
        }
        Command::Check { path } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::check(backend.as_mut(), &path, &root)?)
        }
        Command::Test {
            path,
            changed,
            parse_only,
            output,
        } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::test(
                backend.as_mut(),
                &path,
                &root,
                changed,
                parse_only,
                output.as_deref(),
            )?)
        }
        Command::Project => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::project(backend.as_mut(), &root)?)
        }
        Command::Stats => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::stats(backend.as_mut())?)
        }
        Command::Watch { path, interval } => {
            let mut backend = open(one_shot, &socket)?;
            let mut out = std::io::stdout().lock();
            let mut notes = std::io::stderr().lock();
            watch::run(
                backend.as_mut(),
                &path,
                &root,
                interval,
                &mut out,
                &mut notes,
            )?;
            Ok(0)
        }
        Command::Cache(CacheAction::Gc { dry_run }) => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::cache_gc(backend.as_mut(), &root, dry_run)?)
        }
        Command::Daemon(DaemonAction::Start) => report(lifecycle::start(&socket)?),
        Command::Daemon(DaemonAction::Stop) => report(lifecycle::stop(&socket)?),
        Command::Daemon(DaemonAction::Status) => {
            let (output, code) = lifecycle::status(&socket)?;
            report(output)?;
            Ok(code)
        }
        Command::Daemon(DaemonAction::Serve) => {
            lifecycle::serve(&socket)?;
            Ok(0)
        }
    }
}

/// Pick who answers: a private in-process engine, or the project's daemon - started now if it
/// is not already up, since needing a separate command before the first check would be a
/// papercut on the loop this whole design exists to make fast.
fn open(one_shot: bool, socket: &Path) -> Result<Box<dyn Backend>> {
    if one_shot {
        return Ok(Box::new(OneShot::start()?));
    }
    let mut client = DaemonClient::connect_or_start(socket)?;
    // Fail on version skew before running a command, not half way through one.
    client.handshake()?;
    Ok(Box::new(client))
}

fn report(output: Output) -> Result<i32> {
    emit(&output.text)?;
    for note in &output.notes {
        eprintln!("kiln: {note}");
    }
    Ok(output.exit_code)
}

fn emit(text: &str) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|source| Error::io("cannot write to stdout", source))
}

#[cfg(test)]
mod tests {

    #[test]
    fn arguments_survive_the_trip_through_osstring() {
        let args = utf8_args([OsString::from("check"), OsString::from("a.js")]);
        assert_eq!(args.expect("valid UTF-8 parses"), ["check", "a.js"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_argument_is_a_usage_error_not_a_panic() {
        use std::os::unix::ffi::OsStringExt;

        // A lone 0x80 is a continuation byte with nothing to continue: a legal filename on
        // both macOS and Linux, and not a legal `str`.
        let path = OsString::from_vec(vec![b'a', 0x80, b'.', b'j', b's']);
        let err = utf8_args([OsString::from("check"), path]).expect_err("must not panic");
        assert!(format!("{err}").contains("not valid UTF-8"), "got: {err}");
    }

    use super::*;

    #[test]
    fn help_documents_every_command_and_option() {
        for expected in [
            "run",
            "build",
            "check",
            "test",
            "project",
            "stats",
            "watch",
            "daemon",
            "cache",
            "--one-shot",
            "--socket",
            "--interval",
            "--out",
            "--dry-run",
            "--help",
            "--version",
            "exit status",
        ] {
            assert!(
                args::HELP.contains(expected),
                "`--help` never mentions {expected}"
            );
        }
    }

    #[test]
    fn help_lists_the_current_command_surface() {
        let commands: Vec<&str> = args::HELP
            .lines()
            .skip_while(|line| !line.starts_with("commands:"))
            .skip(1)
            .take_while(|line| line.starts_with("    "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert_eq!(
            commands,
            [
                "run", "build", "check", "test", "project", "stats", "watch", "daemon", "cache"
            ]
        );
    }

    #[test]
    fn version_names_the_protocol_it_speaks() {
        let text = version();
        assert!(text.starts_with("kiln "), "{text}");
        assert!(
            text.contains(&format!("protocol {}", kiln_proto::PROTOCOL_VERSION)),
            "{text}"
        );
    }
}

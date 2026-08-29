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

use std::io::Write;
use std::path::Path;

use args::{Command, DaemonAction, Invocation, Parsed};
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
    match args::parse(command_line()?)? {
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

/// `std::env::args` panics on a non-UTF-8 argument, which a file path can legitimately be.
fn command_line() -> Result<Vec<String>> {
    std::env::args_os()
        .skip(1)
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
        Command::Check { path } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::check(backend.as_mut(), &path)?)?;
            Ok(0)
        }
        Command::Project => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::project(backend.as_mut(), &root)?)?;
            Ok(0)
        }
        Command::Stats => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::stats(backend.as_mut())?)?;
            Ok(0)
        }
        Command::Watch { path, interval } => {
            let mut backend = open(one_shot, &socket)?;
            let mut out = std::io::stdout().lock();
            let mut notes = std::io::stderr().lock();
            watch::run(backend.as_mut(), &path, interval, &mut out, &mut notes)?;
            Ok(0)
        }
        Command::Daemon(DaemonAction::Start) => {
            report(lifecycle::start(&socket)?)?;
            Ok(0)
        }
        Command::Daemon(DaemonAction::Stop) => {
            report(lifecycle::stop(&socket)?)?;
            Ok(0)
        }
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

fn report(output: Output) -> Result<()> {
    emit(&output.text)?;
    for note in &output.notes {
        eprintln!("kiln: {note}");
    }
    Ok(())
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
    use super::*;

    #[test]
    fn help_documents_every_command_and_option() {
        for expected in [
            "check",
            "project",
            "stats",
            "watch",
            "daemon",
            "--one-shot",
            "--socket",
            "--interval",
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
    fn help_does_not_promise_things_v0_cannot_do() {
        // There is no interpreter and no native backend yet, so no command may imply one.
        let commands: Vec<&str> = args::HELP
            .lines()
            .skip_while(|line| !line.starts_with("commands:"))
            .skip(1)
            .take_while(|line| line.starts_with("    "))
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert_eq!(commands, ["check", "project", "stats", "watch", "daemon"]);
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

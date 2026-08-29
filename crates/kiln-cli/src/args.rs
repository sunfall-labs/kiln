//! Hand-rolled argument parsing.
//!
//! Six commands and five flags do not justify a derive-macro dependency on the rebuild path
//! of the tool whose whole pitch is rebuild speed (docs/adr/0004). The parser is a plain
//! loop, and it is strict: a flag that cannot apply to the chosen command is an error rather
//! than something silently ignored.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::{Error, Result};

/// Polling cadence for `kiln watch`. Short enough to feel immediate, long enough that an
/// idle watcher costs nothing measurable.
const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_millis(200);
const MIN_WATCH_INTERVAL_MS: u64 = 10;
const MAX_WATCH_INTERVAL_MS: u64 = 60_000;

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Version,
    Run(Invocation),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    /// Run the pipeline in this process and never contact a daemon.
    pub one_shot: bool,
    /// Overrides the derived per-project socket path.
    pub socket: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Check { path: PathBuf },
    Project,
    Stats,
    Watch { path: PathBuf, interval: Duration },
    Daemon(DaemonAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonAction {
    Start,
    Stop,
    Status,
    /// Run the accept loop in the foreground. This is what `start` and autostart spawn.
    Serve,
}

pub fn parse<I>(args: I) -> Result<Parsed>
where
    I: IntoIterator<Item = String>,
{
    let mut positional: Vec<String> = Vec::new();
    let mut one_shot = false;
    let mut socket: Option<PathBuf> = None;
    let mut interval: Option<Duration> = None;
    let mut only_positional = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if only_positional || !arg.starts_with('-') || arg == "-" {
            positional.push(arg);
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        match flag.as_str() {
            "--" => only_positional = true,
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--one-shot" => {
                reject_inline_value(&flag, &inline)?;
                one_shot = true;
            }
            "--socket" => {
                socket = Some(PathBuf::from(value_for(&flag, inline, &mut args)?));
            }
            "--interval" => {
                let raw = value_for(&flag, inline, &mut args)?;
                interval = Some(parse_interval(&raw)?);
            }
            _ => {
                return Err(Error::usage(format!(
                    "unknown option `{arg}` (try `kiln --help`)"
                )));
            }
        }
    }

    let mut positional = positional.into_iter();
    let Some(name) = positional.next() else {
        return Err(Error::usage("no command given (try `kiln --help`)"));
    };

    let command = match name.as_str() {
        "check" => Command::Check {
            path: PathBuf::from(one_operand(&name, &mut positional)?),
        },
        "project" => {
            no_operands(&name, &mut positional)?;
            Command::Project
        }
        "stats" => {
            no_operands(&name, &mut positional)?;
            Command::Stats
        }
        "watch" => Command::Watch {
            path: PathBuf::from(one_operand(&name, &mut positional)?),
            interval: interval.unwrap_or(DEFAULT_WATCH_INTERVAL),
        },
        "daemon" => {
            let action = one_operand(&name, &mut positional)?;
            let action = match action.as_str() {
                "start" => DaemonAction::Start,
                "stop" => DaemonAction::Stop,
                "status" => DaemonAction::Status,
                "serve" => DaemonAction::Serve,
                other => {
                    return Err(Error::usage(format!(
                        "unknown daemon action `{other}` (expected start, stop, status or serve)"
                    )));
                }
            };
            Command::Daemon(action)
        }
        other => {
            return Err(Error::usage(format!(
                "unknown command `{other}` (try `kiln --help`)"
            )));
        }
    };

    // Flags that cannot mean anything for the chosen command are rejected rather than
    // ignored: silently dropping `--one-shot` would hide which engine actually ran.
    if interval.is_some() && !matches!(command, Command::Watch { .. }) {
        return Err(Error::usage("--interval only applies to `kiln watch`"));
    }
    if one_shot && matches!(command, Command::Daemon(_)) {
        return Err(Error::usage(
            "--one-shot cannot be combined with `kiln daemon`: one-shot mode has no daemon",
        ));
    }

    Ok(Parsed::Run(Invocation {
        command,
        one_shot,
        socket,
    }))
}

fn value_for<I>(flag: &str, inline: Option<String>, rest: &mut I) -> Result<String>
where
    I: Iterator<Item = String>,
{
    match inline.or_else(|| rest.next()) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(Error::usage(format!("{flag} needs a value"))),
    }
}

fn reject_inline_value(flag: &str, inline: &Option<String>) -> Result<()> {
    match inline {
        Some(_) => Err(Error::usage(format!("{flag} does not take a value"))),
        None => Ok(()),
    }
}

fn parse_interval(raw: &str) -> Result<Duration> {
    let ms: u64 = raw
        .parse()
        .map_err(|_| Error::usage(format!("--interval expects milliseconds, got `{raw}`")))?;
    if !(MIN_WATCH_INTERVAL_MS..=MAX_WATCH_INTERVAL_MS).contains(&ms) {
        return Err(Error::usage(format!(
            "--interval must be between {MIN_WATCH_INTERVAL_MS} and {MAX_WATCH_INTERVAL_MS} ms"
        )));
    }
    Ok(Duration::from_millis(ms))
}

fn one_operand<I>(command: &str, rest: &mut I) -> Result<String>
where
    I: Iterator<Item = String>,
{
    let Some(value) = rest.next() else {
        return Err(Error::usage(format!("`kiln {command}` needs an argument")));
    };
    no_operands(command, rest)?;
    Ok(value)
}

fn no_operands<I>(command: &str, rest: &mut I) -> Result<()>
where
    I: Iterator<Item = String>,
{
    match rest.next() {
        Some(extra) => Err(Error::usage(format!(
            "`kiln {command}` takes no extra argument (got `{extra}`)"
        ))),
        None => Ok(()),
    }
}

pub const HELP: &str = "\
kiln - an incremental compiler for JavaScript and TypeScript

kiln does not run or emit code yet. What it does today is drive the incremental
query pipeline and show you what it had to recompute, which is the property the
rest of the compiler is built on.

usage:
    kiln [options] <command>

commands:
    check <file>       check one file and report what was recomputed
    project            summarise the whole project
    stats              print the daemon's recompute counters
    watch <file>       re-check <file> whenever it changes on disk
    daemon <action>    start | stop | status | serve

options:
    --one-shot         run the pipeline in this process; never contact a daemon.
                       Slower per invocation, but reproducible and self-contained,
                       which is what CI wants.
    --socket <path>    daemon socket to use. Defaults to a path under $TMPDIR
                       derived from the working directory, so one daemon serves
                       one project. $KILN_SOCKET sets the same thing.
    --interval <ms>    poll interval for `watch` (default 200, min 10, max 60000)
    -h, --help         show this help
    -V, --version      show version and protocol information

check, project, stats and watch start the daemon if it is not already running:
the everyday loop should not need a separate step.

exit status:
    0    success; for `daemon status`, the daemon is running
    1    failure; for `daemon status`, no daemon is running
    2    the command line was wrong
";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Parsed> {
        parse(args.iter().map(|a| a.to_string()))
    }

    fn run(args: &[&str]) -> Invocation {
        match parse_args(args).expect("should parse") {
            Parsed::Run(invocation) => invocation,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    fn usage_error(args: &[&str]) -> String {
        match parse_args(args) {
            Err(Error::Usage(message)) => message,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn flags_may_precede_or_follow_the_command() {
        let before = run(&["--one-shot", "check", "a.js"]);
        let after = run(&["check", "a.js", "--one-shot"]);
        assert_eq!(before, after);
        assert!(before.one_shot);
        assert_eq!(
            before.command,
            Command::Check {
                path: PathBuf::from("a.js")
            }
        );
    }

    #[test]
    fn socket_accepts_both_spellings() {
        let split = run(&["--socket", "/tmp/k.sock", "stats"]);
        let inline = run(&["--socket=/tmp/k.sock", "stats"]);
        assert_eq!(split, inline);
        assert_eq!(split.socket, Some(PathBuf::from("/tmp/k.sock")));
    }

    #[test]
    fn watch_interval_defaults_and_overrides() {
        let Command::Watch { interval, .. } = run(&["watch", "a.js"]).command else {
            panic!("expected watch");
        };
        assert_eq!(interval, DEFAULT_WATCH_INTERVAL);

        let Command::Watch { interval, .. } = run(&["watch", "a.js", "--interval=50"]).command
        else {
            panic!("expected watch");
        };
        assert_eq!(interval, Duration::from_millis(50));
    }

    #[test]
    fn double_dash_stops_flag_parsing() {
        let invocation = run(&["check", "--", "--weird-name.js"]);
        assert_eq!(
            invocation.command,
            Command::Check {
                path: PathBuf::from("--weird-name.js")
            }
        );
    }

    #[test]
    fn help_and_version_win_over_everything_else() {
        assert_eq!(parse_args(&["nonsense", "--help"]).unwrap(), Parsed::Help);
        assert_eq!(parse_args(&["-V"]).unwrap(), Parsed::Version);
    }

    #[test]
    fn misuse_is_rejected_rather_than_ignored() {
        assert!(usage_error(&[]).contains("no command"));
        assert!(usage_error(&["check"]).contains("needs an argument"));
        assert!(usage_error(&["check", "a.js", "b.js"]).contains("no extra argument"));
        assert!(usage_error(&["stats", "--socket"]).contains("needs a value"));
        assert!(usage_error(&["--one-shot=yes", "stats"]).contains("does not take a value"));
        assert!(usage_error(&["--nope", "stats"]).contains("unknown option"));
        assert!(usage_error(&["compile", "a.js"]).contains("unknown command"));
        assert!(usage_error(&["daemon", "restart"]).contains("unknown daemon action"));
        assert!(usage_error(&["check", "a.js", "--interval=50"]).contains("--interval only"));
        assert!(usage_error(&["--one-shot", "daemon", "start"]).contains("--one-shot cannot"));
        assert!(usage_error(&["watch", "a.js", "--interval=0"]).contains("must be between"));
        assert!(usage_error(&["watch", "a.js", "--interval=soon"]).contains("milliseconds"));
    }
}

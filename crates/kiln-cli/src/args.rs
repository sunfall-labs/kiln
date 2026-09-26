//! Hand-rolled argument parsing.
//!
//! Eight commands and ten flags do not justify a derive-macro dependency on the rebuild path
//! of the tool whose whole pitch is rebuild speed (docs/adr/0004). The parser is a plain
//! loop, and it is strict: a flag that cannot apply to the chosen command is an error rather
//! than something silently ignored.

use std::path::PathBuf;
use std::time::Duration;

use crate::error::{Error, Result};
use kiln_proto::{NativeProfile, StepBudget};

// `--interval` outlived the poll it was written for, but it now names the debounce window
// rather than a cadence. Dropping it was the alternative, and it was tempting: one fewer
// flag, one fewer thing to document. It loses, because the window is the entire delay
// `watch` still adds on purpose between a save and an answer, charged against the 200ms
// design.md §13 budgets, and hiding the only remaining cost of the loop inside a constant is
// the opposite of what the rest of this CLI does. It is also the one number a user on a
// filesystem that reports a save as a longer burst may genuinely have to raise. So it stays,
// with a new meaning, a new range and new help text: the old cadence values are mostly
// errors now, which is the right way for a meaning to change.

/// Debounce window for `kiln watch`: how long the file must stop changing before it is
/// re-checked. An editor's atomic save arrives as a burst within a millisecond or two, so
/// this only has to be long enough to swallow the burst - and it is paid in full on every
/// save, so it is kept far below the budget it spends from.
const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_millis(25);
/// Under a millisecond there is nothing left to coalesce; over a couple of seconds the
/// watcher feels broken, which is a mistake rather than a preference.
const MIN_WATCH_INTERVAL_MS: u64 = 1;
const MAX_WATCH_INTERVAL_MS: u64 = 2_000;

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
    Check {
        path: PathBuf,
        node: bool,
    },
    Run {
        path: PathBuf,
        args: Vec<String>,
        node: bool,
        native: bool,
        profile: NativeProfile,
        /// How long the program may run. Unmetered unless `--max-steps` says otherwise: a
        /// person running their own program is not the caller a step budget protects.
        steps: StepBudget,
        /// Number of concurrent worker realms / threads sharing the listening socket.
        workers: usize,
        march_native: bool,
        static_link: bool,
        pgo_generate: Option<PathBuf>,
        pgo_use: Option<PathBuf>,
    },
    Build {
        path: PathBuf,
        output: Option<PathBuf>,
        profile: NativeProfile,
        shared: bool,
        node: bool,
        march_native: bool,
        explain: bool,
        /// Build a binary that never installs kiln's own members (ADR 0068).
        no_kiln_extensions: bool,
        static_link: bool,
        pgo_generate: Option<PathBuf>,
        pgo_use: Option<PathBuf>,
    },
    Test {
        path: PathBuf,
        changed: bool,
        parse_only: bool,
        node: bool,
        output: Option<PathBuf>,
    },
    Project,
    Stats,
    Watch {
        path: PathBuf,
        interval: Duration,
        node: bool,
    },
    Daemon(DaemonAction),
    Cache(CacheAction),
    Exec {
        binary: String,
        args: Vec<String>,
    },
}

/// What `kiln cache` was asked to do. One action today, spelled as an action rather than as a
/// `kiln gc` command because the cache is the noun a later `cache size` or `cache path` would
/// also belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAction {
    /// Sweep the project's artifact cache back under its byte budget.
    Gc { dry_run: bool },
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
    let mut changed = false;
    let mut parse_only = false;
    let mut node = false;
    let mut native = false;
    let mut output: Option<PathBuf> = None;
    let mut test_output: Option<PathBuf> = None;
    let mut profile: Option<NativeProfile> = None;
    let mut max_steps: Option<u64> = None;
    let mut workers: Option<usize> = None;
    let mut shared = false;
    let mut march_native = false;
    let mut explain = false;
    let mut no_kiln_extensions = false;
    let mut static_link = false;
    let mut pgo_generate: Option<PathBuf> = None;
    let mut pgo_use: Option<PathBuf> = None;
    let mut dry_run = false;
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
            "-h" | "--help" => {
                if positional.len() >= 2 && positional[0] == "run" {
                    positional.push(arg);
                } else {
                    return Ok(Parsed::Help);
                }
            }
            "-V" | "--version" => {
                if positional.len() >= 2 && positional[0] == "run" {
                    positional.push(arg);
                } else {
                    return Ok(Parsed::Version);
                }
            }
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
            "--changed" => {
                reject_inline_value(&flag, &inline)?;
                changed = true;
            }
            "--parse-only" => {
                reject_inline_value(&flag, &inline)?;
                parse_only = true;
            }
            "--node" => {
                reject_inline_value(&flag, &inline)?;
                node = true;
            }
            "--native" => {
                reject_inline_value(&flag, &inline)?;
                native = true;
            }
            "--shared" => {
                reject_inline_value(&flag, &inline)?;
                shared = true;
            }
            "--dry-run" => {
                reject_inline_value(&flag, &inline)?;
                dry_run = true;
            }
            "--output" => {
                output = Some(PathBuf::from(value_for(&flag, inline, &mut args)?));
            }
            "--out" => {
                test_output = Some(PathBuf::from(value_for(&flag, inline, &mut args)?));
            }
            "--profile" => {
                let raw = value_for(&flag, inline, &mut args)?;
                profile = Some(parse_profile(&raw)?);
            }
            "--march" => {
                let raw = value_for(&flag, inline, &mut args)?;
                if raw == "native" {
                    march_native = true;
                } else {
                    return Err(Error::usage(format!(
                        "--march expects `native`, got `{raw}`"
                    )));
                }
            }
            "--max-steps" => {
                let raw = value_for(&flag, inline, &mut args)?;
                max_steps = Some(parse_max_steps(&raw)?);
            }
            "--workers" => {
                let raw = value_for(&flag, inline, &mut args)?;
                let count: usize = raw.parse().map_err(|_| {
                    Error::usage(format!(
                        "invalid worker count `{raw}`: expected a positive integer"
                    ))
                })?;
                if count == 0 {
                    return Err(Error::usage("worker count must be at least 1"));
                }
                workers = Some(count);
            }
            "--explain" => {
                explain = true;
            }
            "--no-kiln-extensions" => {
                reject_inline_value(&flag, &inline)?;
                no_kiln_extensions = true;
            }
            "--static" => {
                reject_inline_value(&flag, &inline)?;
                static_link = true;
            }
            "--pgo-generate" => {
                pgo_generate = Some(PathBuf::from(value_for(&flag, inline, &mut args)?));
            }
            "--pgo-use" => {
                pgo_use = Some(PathBuf::from(value_for(&flag, inline, &mut args)?));
            }
            _ => {
                if positional.len() >= 2 && positional[0] == "run" {
                    positional.push(arg);
                } else {
                    return Err(Error::usage(format!("unknown option `{arg}`")));
                }
            }
        }
    }

    let mut positional = positional.into_iter();
    let Some(name) = positional.next() else {
        return Err(Error::usage("no command given"));
    };

    if native && name != "run" {
        return Err(Error::usage("--native only applies to `kiln run`"));
    }
    if shared && name != "build" {
        return Err(Error::usage("--shared only applies to `kiln build`"));
    }
    if dry_run && name != "cache" {
        return Err(Error::usage("--dry-run only applies to `kiln cache gc`"));
    }
    if output.is_some() && name != "build" {
        return Err(Error::usage("--output only applies to `kiln build`"));
    }
    if test_output.is_some() && name != "test" {
        return Err(Error::usage("--out only applies to `kiln test`"));
    }
    if test_output.is_some() && changed {
        return Err(Error::usage(
            "--out cannot be combined with `kiln test --changed`: results must cover the full corpus",
        ));
    }
    if test_output.is_some() && parse_only {
        return Err(Error::usage(
            "--out cannot be combined with `kiln test --parse-only`: results must be from an interpreter run",
        ));
    }
    if node
        && name != "test"
        && name != "run"
        && name != "build"
        && name != "watch"
        && name != "check"
    {
        return Err(Error::usage(
            "--node only applies to `kiln run`, `kiln build`, `kiln test`, `kiln watch`, or `kiln check`",
        ));
    }
    if node && shared {
        return Err(Error::usage(
            "--node cannot be combined with `kiln build --shared`",
        ));
    }
    if node && parse_only {
        return Err(Error::usage(
            "--node cannot be combined with `kiln test --parse-only`",
        ));
    }
    if profile.is_some() && name != "build" && !native {
        return Err(Error::usage(
            "--profile only applies to native `kiln run` or `kiln build`",
        ));
    }
    if march_native && name != "build" && !native {
        return Err(Error::usage(
            "--march=native only applies to native `kiln run` or `kiln build`",
        ));
    }
    if static_link && name != "build" && !native {
        return Err(Error::usage(
            "--static only applies to native `kiln run` or `kiln build`",
        ));
    }
    if pgo_generate.is_some() && name != "build" && !native {
        return Err(Error::usage(
            "--pgo-generate only applies to native `kiln run` or `kiln build`",
        ));
    }
    if pgo_use.is_some() && name != "build" && !native {
        return Err(Error::usage(
            "--pgo-use only applies to native `kiln run` or `kiln build`",
        ));
    }
    if pgo_generate.is_some() && pgo_use.is_some() {
        return Err(Error::usage(
            "--pgo-generate and --pgo-use cannot be used together",
        ));
    }
    if max_steps.is_some() && name != "run" {
        return Err(Error::usage(
            "--max-steps only applies to interpreted `kiln run`",
        ));
    }
    if workers.is_some() && name != "run" {
        return Err(Error::usage("--workers only applies to `kiln run`"));
    }
    if explain && name != "build" {
        return Err(Error::usage("--explain only applies to `kiln build`"));
    }
    // The interpreter and `kiln run --native` take the same choice from `KILN_EXTENSIONS=0` at run
    // time; the flag exists for the one thing an environment cannot do, which is fix the answer
    // into a binary that will be run somewhere else (ADR 0068).
    if no_kiln_extensions && name != "build" {
        return Err(Error::usage(
            "--no-kiln-extensions only applies to `kiln build`; set KILN_EXTENSIONS=0 to run without them",
        ));
    }
    if no_kiln_extensions && shared {
        return Err(Error::usage(
            "--no-kiln-extensions cannot be combined with `kiln build --shared`",
        ));
    }
    // The native backend compiles the step counter out entirely, so a budget there would be a
    // flag that silently does nothing -- which is the one thing this parser refuses to be.
    if max_steps.is_some() && native {
        return Err(Error::usage(
            "--max-steps cannot be combined with `kiln run --native`: the native backend does not count steps",
        ));
    }

    let command = match name.as_str() {
        "check" => Command::Check {
            path: PathBuf::from(one_operand(&name, &mut positional)?),
            node,
        },
        "run" => {
            let Some(path) = positional.next() else {
                return Err(Error::usage("`kiln run` needs an argument"));
            };
            let args: Vec<String> = positional.collect();
            Command::Run {
                path: PathBuf::from(path),
                args,
                node,
                native,
                profile: profile.unwrap_or_default(),
                steps: max_steps.map_or(StepBudget::Unmetered, StepBudget::Limited),
                workers: workers.unwrap_or(1),
                march_native,
                static_link,
                pgo_generate,
                pgo_use,
            }
        }
        "build" => Command::Build {
            path: PathBuf::from(one_operand(&name, &mut positional)?),
            output,
            profile: profile.unwrap_or_default(),
            shared,
            node,
            march_native,
            explain,
            no_kiln_extensions,
            static_link,
            pgo_generate,
            pgo_use,
        },
        "test" => Command::Test {
            path: PathBuf::from(one_operand(&name, &mut positional)?),
            changed,
            parse_only,
            node,
            output: test_output,
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
            node,
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
        "cache" => {
            let action = one_operand(&name, &mut positional)?;
            let action = match action.as_str() {
                "gc" => CacheAction::Gc { dry_run },
                other => {
                    return Err(Error::usage(format!(
                        "unknown cache action `{other}` (expected gc)"
                    )));
                }
            };
            Command::Cache(action)
        }
        "x" | "exec" => {
            let Some(binary) = positional.next() else {
                return Err(Error::usage("no binary specified for `kiln x`"));
            };
            let args = positional.collect();
            Command::Exec { binary, args }
        }
        other => {
            return Err(Error::usage(format!("unknown command `{other}`")));
        }
    };

    // Flags that cannot mean anything for the chosen command are rejected rather than
    // ignored: silently dropping `--one-shot` would hide which engine actually ran.
    if interval.is_some() && !matches!(command, Command::Watch { .. }) {
        return Err(Error::usage("--interval only applies to `kiln watch`"));
    }
    if changed && !matches!(command, Command::Test { .. }) {
        return Err(Error::usage("--changed only applies to `kiln test`"));
    }
    if parse_only && !matches!(command, Command::Test { .. }) {
        return Err(Error::usage("--parse-only only applies to `kiln test`"));
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

/// `--max-steps <n>`: the bound a caller wants imposed on a program that may not terminate.
///
/// Zero is rejected rather than treated as "no bound", because a budget of nothing is a request
/// nobody means and the absence of the flag already says the other thing.
fn parse_max_steps(raw: &str) -> Result<u64> {
    match raw.parse::<u64>() {
        Ok(0) | Err(_) => Err(Error::usage(format!(
            "--max-steps expects a positive number of interpreter steps, got `{raw}`"
        ))),
        Ok(steps) => Ok(steps),
    }
}

fn parse_profile(raw: &str) -> Result<NativeProfile> {
    match raw {
        "native-dev" => Ok(NativeProfile::Dev),
        "native-check" => Ok(NativeProfile::Check),
        "native-release" => Ok(NativeProfile::Release),
        "native-release-size" | "native-size" => Ok(NativeProfile::ReleaseSize),
        "native-profile" => Ok(NativeProfile::Profile),
        _ => Err(Error::usage(format!(
            "--profile expects native-dev, native-check, native-release, native-release-size, or native-profile, got `{raw}`"
        ))),
    }
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

kiln runs a deliberately growing JavaScript/TypeScript subset through an incremental
compiler and interpreter. `check` accepts the full syntax understood by the frontend;
`run` reports source-ranged diagnostics for execution features not implemented yet.

usage:
    kiln [options] <command>

commands:
    run <file>         compile and execute one module (or use --native)
    build <file>       compile and link a native executable or shared library
    check <file>       check one file and report what was recomputed
    test <directory>   run a Test262 tree or Kiln's local conformance corpus
    project            summarise the whole project
    stats              print the daemon's recompute counters
    watch <file>       run <file> now and rerun it whenever its bytes change on disk
    daemon <action>    start | stop | status | serve
    cache <action>     gc
    x <binary> [args]  execute a package binary from node_modules/.bin or PATH

options:
    --one-shot         run the pipeline in this process; never contact a daemon.
                       Slower per invocation, but reproducible and self-contained,
                       which is what CI wants.
    --socket <path>    daemon socket to use. Defaults to a path under $TMPDIR
                       derived from the working directory, so one daemon serves
                       one project. $KILN_SOCKET sets the same thing.
    --interval <ms>    how long `watch` waits for a file to stop changing before it
                       reruns it (default 25, min 1, max 2000). `watch` is
                       event-driven, so this coalesces one save's burst of writes;
                       it is not a polling cadence.
    --changed          with `test`, run failures, smoke, and affected tests only
    --parse-only       with `test`, classify parser outcomes without interpreting
    --node             with `run`, `build`, or `test`, install the Tokio-owned Node compatibility Adapter
    --native           with `run`, execute through the native backend
    --shared            with `build`, write a shared library for embedding
    --dry-run          with `cache gc`, report the sweep without deleting anything
    --output <path>    with `build`, write the native executable or shared library here
    --out <path>       with `test`, write schema-1 conformance results here
    --profile <name>   native-dev, native-check, native-release, native-release-size,
                       or native-profile (native-profile is native-release plus gprof
                       instrumentation; see scripts/profile-native.sh)
    --march=native     with `build` or native `run`, generate host-specific CPU
                       instructions rather than a portable baseline
    --static           with `build` or native `run`, link a static binary (Linux only)
    --pgo-generate <dir> with `build` or native `run`, instrument binary to generate PGO profiles
    --pgo-use <path>   with `build` or native `run`, optimize binary using Clang PGO profile
    --max-steps <n>    with `run`, refuse after n interpreter steps. Unbounded by
                       default: a program you are running yourself is not one this
                       tool needs protecting from. The conformance corpus keeps its
                       own bound, which is where the guarantee is actually needed.
    -h, --help         show this help
    -V, --version      show version and protocol information

run, check, test, project, stats, watch and cache gc start the daemon if it is not already
running: the everyday loop should not need a separate step.

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
                path: PathBuf::from("a.js"),
                node: false,
            }
        );
    }

    #[test]
    fn native_build_and_run_profiles_parse() {
        assert_eq!(
            run(&[
                "build",
                "src/main.ts",
                "--output",
                "bin/app",
                "--profile=native-release"
            ])
            .command,
            Command::Build {
                path: PathBuf::from("src/main.ts"),
                output: Some(PathBuf::from("bin/app")),
                profile: NativeProfile::Release,
                shared: false,
                node: false,
                march_native: false,
                explain: false,
                no_kiln_extensions: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
        assert!(matches!(
            run(&["build", "src/main.ts", "--shared"]).command,
            Command::Build {
                shared: true,
                profile: NativeProfile::Dev,
                ..
            }
        ));
        assert_eq!(
            run(&[
                "run",
                "src/main.ts",
                "--native",
                "--profile",
                "native-check"
            ])
            .command,
            Command::Run {
                path: PathBuf::from("src/main.ts"),
                args: Vec::new(),
                node: false,
                native: true,
                profile: NativeProfile::Check,
                steps: StepBudget::Unmetered,
                workers: 1,
                march_native: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
    }

    #[test]
    fn shared_only_applies_to_build() {
        assert!(usage_error(&["run", "a.js", "--shared"]).contains("only applies"));
        assert!(usage_error(&["check", "a.js", "--shared"]).contains("only applies"));
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
    fn watch_accepts_node_mode() {
        assert_eq!(
            run(&["watch", "app.mjs", "--node"]).command,
            Command::Watch {
                path: PathBuf::from("app.mjs"),
                interval: DEFAULT_WATCH_INTERVAL,
                node: true,
            }
        );
    }

    #[test]
    fn test_mode_options_are_explicit_and_scoped() {
        assert_eq!(
            run(&["test", "test262", "--changed", "--parse-only"]).command,
            Command::Test {
                path: PathBuf::from("test262"),
                changed: true,
                parse_only: true,
                node: false,
                output: None,
            }
        );
        assert_eq!(
            run(&["test", "test262", "--out=target/results.json"]).command,
            Command::Test {
                path: PathBuf::from("test262"),
                changed: false,
                parse_only: false,
                node: false,
                output: Some(PathBuf::from("target/results.json")),
            }
        );
        assert!(usage_error(&["check", "a.js", "--changed"]).contains("only applies"));
        assert!(usage_error(&["run", "a.js", "--parse-only"]).contains("only applies"));
        assert!(matches!(
            run(&["run", "a.mjs", "--node"]).command,
            Command::Run {
                node: true,
                native: false,
                ..
            }
        ));
        assert!(
            usage_error(&["test", "test262", "--node", "--parse-only"])
                .contains("cannot be combined")
        );
        assert!(usage_error(&["check", "a.js", "--out", "result.json"]).contains("only applies"));
        assert!(
            usage_error(&["test", "test262", "--changed", "--out", "result.json"])
                .contains("full corpus")
        );
        assert!(
            usage_error(&["test", "test262", "--parse-only", "--out", "result.json"])
                .contains("interpreter run")
        );
    }

    /// The step budget is the caller's to choose, and `kiln run` chooses none.
    ///
    /// `ExecutionLimits::default()` bounds a run at a million steps so a corpus probe that loops
    /// forever fails instead of hanging the runner. That bound reached `kiln run` too, where
    /// nobody is queued behind the program and node imposes none -- and the smallest schema
    /// `ajv` can compile exceeds it on its own, so it answered "too long" to a program that was
    /// not.
    #[test]
    fn a_run_is_unmetered_unless_the_caller_asks_for_a_bound() {
        assert!(matches!(
            run(&["run", "a.mjs"]).command,
            Command::Run {
                steps: StepBudget::Unmetered,
                ..
            }
        ));
        assert!(matches!(
            run(&["run", "a.mjs", "--max-steps", "5000"]).command,
            Command::Run {
                steps: StepBudget::Limited(5000),
                ..
            }
        ));
        assert!(matches!(
            run(&["run", "a.mjs", "--max-steps=5000"]).command,
            Command::Run {
                steps: StepBudget::Limited(5000),
                ..
            }
        ));
        // A budget of nothing is a request nobody means, and the absence of the flag already
        // says the other thing.
        assert!(usage_error(&["run", "a.mjs", "--max-steps", "0"]).contains("positive number"));
        assert!(usage_error(&["run", "a.mjs", "--max-steps", "many"]).contains("positive number"));
        assert!(usage_error(&["check", "a.mjs", "--max-steps", "10"]).contains("only applies"));
        // Refused rather than accepted and ignored: the native backend does not count steps.
        assert!(
            usage_error(&["run", "a.mjs", "--native", "--max-steps", "10"])
                .contains("cannot be combined")
        );
    }

    #[test]
    fn workers_option_defaults_and_overrides() {
        assert!(matches!(
            run(&["run", "server.mjs"]).command,
            Command::Run { workers: 1, .. }
        ));
        assert!(matches!(
            run(&["run", "server.mjs", "--workers", "4"]).command,
            Command::Run { workers: 4, .. }
        ));
        assert!(matches!(
            run(&["run", "server.mjs", "--workers=8"]).command,
            Command::Run { workers: 8, .. }
        ));
        assert!(usage_error(&["run", "server.mjs", "--workers", "0"]).contains("at least 1"));
        assert!(
            usage_error(&["run", "server.mjs", "--workers", "abc"]).contains("positive integer")
        );
        assert!(usage_error(&["check", "server.mjs", "--workers", "4"]).contains("only applies"));
    }

    #[test]
    fn node_run_is_explicit_and_composes_with_native() {
        assert!(matches!(
            run(&["run", "server.mjs", "--node"]).command,
            Command::Run {
                node: true,
                native: false,
                ..
            }
        ));
        // Node policy and execution backend remain independent explicit choices.
        assert!(matches!(
            run(&["run", "server.mjs", "--node", "--native"]).command,
            Command::Run {
                node: true,
                native: true,
                ..
            }
        ));
    }

    #[test]
    fn node_build_is_explicit_and_shared_node_builds_fail_closed() {
        assert!(matches!(
            run(&["build", "server.mjs", "--node"]).command,
            Command::Build {
                node: true,
                shared: false,
                ..
            }
        ));
        assert!(
            usage_error(&["build", "server.mjs", "--node", "--shared"])
                .contains("cannot be combined")
        );
    }

    #[test]
    fn cache_gc_parses_its_one_action_and_its_one_flag() {
        assert_eq!(
            run(&["cache", "gc"]).command,
            Command::Cache(CacheAction::Gc { dry_run: false })
        );
        assert_eq!(
            run(&["cache", "gc", "--dry-run"]).command,
            Command::Cache(CacheAction::Gc { dry_run: true })
        );
        assert_eq!(
            run(&["--dry-run", "cache", "gc"]).command,
            Command::Cache(CacheAction::Gc { dry_run: true })
        );
    }

    #[test]
    fn cache_misuse_is_rejected_rather_than_ignored() {
        assert!(usage_error(&["cache"]).contains("needs an argument"));
        assert!(usage_error(&["cache", "clear"]).contains("unknown cache action"));
        assert!(usage_error(&["cache", "gc", "extra"]).contains("no extra argument"));
        // A `--dry-run` that quietly did nothing on `build` would be the worst possible
        // reading of the flag: the user would believe a destructive command was rehearsed.
        assert!(usage_error(&["build", "a.js", "--dry-run"]).contains("--dry-run only applies"));
        assert!(usage_error(&["cache", "gc", "--dry-run=yes"]).contains("does not take a value"));
    }

    #[test]
    fn double_dash_stops_flag_parsing() {
        let invocation = run(&["check", "--", "--weird-name.js"]);
        assert_eq!(
            invocation.command,
            Command::Check {
                path: PathBuf::from("--weird-name.js"),
                node: false,
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
        // A debounce window that long is a hung watcher, not a preference - and it is what
        // the flag used to accept, so rejecting it is how the change of meaning gets noticed.
        assert!(usage_error(&["watch", "a.js", "--interval=60000"]).contains("must be between"));
        assert!(usage_error(&["watch", "a.js", "--interval=soon"]).contains("milliseconds"));
        assert!(usage_error(&["run", "a.js", "--profile=native-dev"]).contains("only applies"));
        assert!(usage_error(&["check", "a.js", "--native"]).contains("only applies"));
        assert!(usage_error(&["build", "a.js", "--profile=debug"]).contains("expects"));
        assert!(usage_error(&["run", "a.js", "--march=native"]).contains("only applies"));
        assert!(usage_error(&["check", "a.js", "--march=native"]).contains("only applies"));
        assert!(usage_error(&["build", "a.js", "--march=x86-64"]).contains("expects `native`"));
    }

    #[test]
    fn march_native_option_parses_for_build_and_native_run() {
        assert_eq!(
            run(&["build", "src/main.ts", "--march=native"]).command,
            Command::Build {
                path: PathBuf::from("src/main.ts"),
                output: None,
                profile: NativeProfile::Dev,
                shared: false,
                node: false,
                march_native: true,
                explain: false,
                no_kiln_extensions: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
        assert_eq!(
            run(&["build", "src/main.ts", "--march", "native"]).command,
            Command::Build {
                path: PathBuf::from("src/main.ts"),
                output: None,
                profile: NativeProfile::Dev,
                shared: false,
                node: false,
                march_native: true,
                explain: false,
                no_kiln_extensions: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
        assert_eq!(
            run(&["run", "src/main.ts", "--native", "--march=native"]).command,
            Command::Run {
                path: PathBuf::from("src/main.ts"),
                args: Vec::new(),
                node: false,
                native: true,
                profile: NativeProfile::Dev,
                steps: StepBudget::Unmetered,
                workers: 1,
                march_native: true,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
    }

    #[test]
    fn no_kiln_extensions_parses_for_build_only() {
        assert!(matches!(
            run(&["build", "src/main.ts", "--no-kiln-extensions"]).command,
            Command::Build {
                no_kiln_extensions: true,
                ..
            }
        ));
        assert!(matches!(
            run(&["build", "src/main.ts"]).command,
            Command::Build {
                no_kiln_extensions: false,
                ..
            }
        ));
        assert!(
            usage_error(&["run", "a.js", "--no-kiln-extensions"]).contains("KILN_EXTENSIONS=0")
        );
        assert!(
            usage_error(&["build", "a.js", "--shared", "--no-kiln-extensions"])
                .contains("--shared")
        );
    }

    #[test]
    fn explain_option_parses_for_build() {
        assert_eq!(
            run(&["build", "src/main.ts", "--explain"]).command,
            Command::Build {
                path: PathBuf::from("src/main.ts"),
                output: None,
                profile: NativeProfile::Dev,
                shared: false,
                node: false,
                march_native: false,
                explain: true,
                no_kiln_extensions: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
        assert!(usage_error(&["run", "a.js", "--explain"]).contains("only applies"));
    }

    #[test]
    fn static_and_pgo_options_parse_for_build_and_native_run() {
        assert_eq!(
            run(&["build", "src/main.ts", "--static"]).command,
            Command::Build {
                path: PathBuf::from("src/main.ts"),
                output: None,
                profile: NativeProfile::Dev,
                shared: false,
                node: false,
                march_native: false,
                explain: false,
                no_kiln_extensions: false,
                static_link: true,
                pgo_generate: None,
                pgo_use: None,
            }
        );
        assert_eq!(
            run(&[
                "run",
                "src/main.ts",
                "--native",
                "--static",
                "--pgo-generate",
                "/tmp/pgo"
            ])
            .command,
            Command::Run {
                path: PathBuf::from("src/main.ts"),
                args: Vec::new(),
                node: false,
                native: true,
                profile: NativeProfile::Dev,
                steps: StepBudget::Unmetered,
                workers: 1,
                march_native: false,
                static_link: true,
                pgo_generate: Some(PathBuf::from("/tmp/pgo")),
                pgo_use: None,
            }
        );

        assert_eq!(
            run(&[
                "build",
                "src/main.ts",
                "--pgo-use=/tmp/pgo/profile.profdata"
            ])
            .command,
            Command::Build {
                path: PathBuf::from("src/main.ts"),
                output: None,
                profile: NativeProfile::Dev,
                shared: false,
                node: false,
                march_native: false,
                explain: false,
                no_kiln_extensions: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: Some(PathBuf::from("/tmp/pgo/profile.profdata")),
            }
        );
        assert!(usage_error(&["run", "a.js", "--static"]).contains("only applies"));
        assert!(usage_error(&["run", "a.js", "--pgo-generate", "dir"]).contains("only applies"));
        assert!(usage_error(&["run", "a.js", "--pgo-use", "file"]).contains("only applies"));
        assert!(
            usage_error(&[
                "build",
                "a.js",
                "--pgo-generate",
                "dir",
                "--pgo-use",
                "file"
            ])
            .contains("cannot be used together")
        );
    }

    #[test]
    fn run_forwards_arguments_after_script_path() {
        assert_eq!(
            run(&["run", "--node", "cli.ts", "--version"]).command,
            Command::Run {
                path: PathBuf::from("cli.ts"),
                args: vec!["--version".to_string()],
                node: true,
                native: false,
                profile: NativeProfile::Dev,
                steps: StepBudget::Unmetered,
                workers: 1,
                march_native: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
        assert_eq!(
            run(&["run", "app.ts", "cmd", "--flag", "val"]).command,
            Command::Run {
                path: PathBuf::from("app.ts"),
                args: vec!["cmd".to_string(), "--flag".to_string(), "val".to_string()],
                node: false,
                native: false,
                profile: NativeProfile::Dev,
                steps: StepBudget::Unmetered,
                workers: 1,
                march_native: false,
                static_link: false,
                pgo_generate: None,
                pgo_use: None,
            }
        );
    }
}

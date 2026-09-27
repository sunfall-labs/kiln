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
mod env;
mod error;
mod exec;
mod lifecycle;
mod oneshot;
mod render;
mod sources;
mod watch;

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    let mut raw_args = utf8_args(std::env::args_os())?;
    let program = raw_args
        .first()
        .and_then(|p| Path::new(p).file_name()?.to_str())
        .unwrap_or("");
    let is_kilnx = program == "kilnx" || program.starts_with("kilnx-");
    let args = if is_kilnx {
        if raw_args.len() > 1 {
            raw_args.remove(0);
            raw_args.insert(0, "x".to_string());
            raw_args
        } else {
            vec!["--help".to_string()]
        }
    } else {
        raw_args.into_iter().skip(1).collect()
    };

    match args::parse(args)? {
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

fn target_for_command(command: &Command) -> Option<&Path> {
    match command {
        Command::Run { path, .. }
        | Command::Build { path, .. }
        | Command::Check { path, .. }
        | Command::Watch { path, .. } => Some(path.as_path()),
        _ => None,
    }
}

fn is_workspace_root(dir: &Path) -> bool {
    if dir.join("pnpm-workspace.yaml").is_file() {
        return true;
    }
    let manifest_path = dir.join("package.json");
    if let Ok(content) = std::fs::read_to_string(&manifest_path)
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content)
        && parsed.get("workspaces").is_some()
    {
        return true;
    }
    false
}

fn find_project_root(target: Option<&Path>) -> Result<PathBuf> {
    let current_dir = std::env::current_dir()
        .map_err(|source| Error::io("cannot read the working directory", source))?;

    let Some(target) = target else {
        return Ok(current_dir);
    };

    let target_abs = if target.is_absolute() {
        target.to_path_buf()
    } else {
        current_dir.join(target)
    };

    let start_dir = match target_abs.canonicalize() {
        Ok(path) => {
            if path.is_dir() {
                path
            } else {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            }
        }
        Err(_) => {
            if target_abs.is_dir() {
                target_abs
            } else {
                target_abs
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or(target_abs)
            }
        }
    };

    let mut current = Some(start_dir.as_path());
    let mut candidate = None;

    while let Some(dir) = current {
        if is_workspace_root(dir) {
            return Ok(dir.to_path_buf());
        }
        if candidate.is_none()
            && (dir.join("package.json").is_file()
                || dir.join("tsconfig.json").is_file()
                || dir.join("jsconfig.json").is_file()
                || dir.join("node_modules").is_dir())
        {
            candidate = Some(dir.to_path_buf());
        }
        // A repository is a project boundary, and the search stops at one rather than treating
        // it as a fallback to be used only if nothing above matched. It was a fallback, and a
        // `node_modules` *above* the repository therefore captured the root: with one at
        // `~/Developer/Personal`, running a file inside `~/Developer/Personal/kiln` made the
        // project `~/Developer/Personal` -- so the walk covered every sibling checkout, hit its
        // ten-thousand-file ceiling, and printed a truncation note on stderr for every run.
        //
        // That note is what turned the thirdparty gate red: it compares stderr against node's,
        // and node says nothing. The bundles themselves were fine, which is why all nineteen
        // "diverged" with identical stdout.
        //
        // A `node_modules` that belongs to this project is still found, because it is at or
        // below the repository root and so is reached before this returns.
        if dir.join(".git").exists() {
            return Ok(candidate.unwrap_or_else(|| dir.to_path_buf()));
        }
        current = dir.parent();
    }

    Ok(candidate.unwrap_or(start_dir))
}

fn dispatch(invocation: Invocation) -> Result<i32> {
    let target = target_for_command(&invocation.command);
    let root = find_project_root(target)?;
    let socket = client::socket_path(invocation.socket, &root)?;
    let one_shot = invocation.one_shot;

    let is_test = matches!(invocation.command, Command::Test { .. });
    env::load_project_env(&root, is_test);
    // `KILN_EXTENSIONS=0` (ADR 0068) is a property of the process that runs the program, and both
    // engines read it once per process. A resident daemon was started with its own environment,
    // so a command that asks for the switch is served in this process instead, where it applies.
    let one_shot = one_shot || !kiln_builtin_surface::kiln_extensions_installed();

    let auto_node = |explicit: bool| -> bool {
        explicit
            || root.join("package.json").is_file()
            || root.join("tsconfig.json").is_file()
            || root.join("jsconfig.json").is_file()
            || root.join("node_modules").is_dir()
    };

    match invocation.command {
        Command::Run {
            path,
            args,
            node,
            native: false,
            steps,
            workers,
            ..
        } => {
            let node = auto_node(node);
            if workers > 1 {
                let mut handles = Vec::with_capacity(workers);
                for _ in 0..workers {
                    let path = path.clone();
                    let args = args.clone();
                    let root = root.clone();
                    let socket = socket.clone();
                    handles.push(std::thread::spawn(move || {
                        let mut backend = open(one_shot, &socket)?;
                        commands::run(backend.as_mut(), &path, &args, &root, node, steps)
                    }));
                }
                let mut status = 0;
                for handle in handles {
                    match handle.join() {
                        Ok(Ok(output)) => {
                            let worker_status = report(output)?;
                            if worker_status != 0 {
                                status = worker_status;
                            }
                        }
                        Ok(Err(err)) => return Err(err),
                        Err(_) => return Err(Error::Protocol("worker thread panicked".to_owned())),
                    }
                }
                Ok(status)
            } else {
                let mut backend = open(one_shot, &socket)?;
                report(commands::run(
                    backend.as_mut(),
                    &path,
                    &args,
                    &root,
                    node,
                    steps,
                )?)
            }
        }
        Command::Run {
            path,
            args,
            node,
            native: true,
            profile,
            // Refused by the parser: the native backend does not count steps.
            steps: _,
            workers,
            march_native: _,
            static_link,
            pgo_generate,
            pgo_use,
        } => {
            let node = auto_node(node);
            if workers > 1 {
                let mut handles = Vec::with_capacity(workers);
                for _ in 0..workers {
                    let path = path.clone();
                    let args = args.clone();
                    let root = root.clone();
                    let socket = socket.clone();
                    let pgo_gen = pgo_generate.clone();
                    let pgo_u = pgo_use.clone();
                    handles.push(std::thread::spawn(move || {
                        let mut backend = open(one_shot, &socket)?;
                        commands::native_run(
                            backend.as_mut(),
                            &path,
                            &root,
                            &args,
                            profile,
                            node,
                            static_link,
                            pgo_gen.as_deref(),
                            pgo_u.as_deref(),
                        )
                    }));
                }
                let mut status = 0;
                for handle in handles {
                    match handle.join() {
                        Ok(Ok(output)) => {
                            let worker_status = report(output)?;
                            if worker_status != 0 {
                                status = worker_status;
                            }
                        }
                        Ok(Err(err)) => return Err(err),
                        Err(_) => return Err(Error::Protocol("worker thread panicked".to_owned())),
                    }
                }
                Ok(status)
            } else {
                let mut backend = open(one_shot, &socket)?;
                report(commands::native_run(
                    backend.as_mut(),
                    &path,
                    &root,
                    &args,
                    profile,
                    node,
                    static_link,
                    pgo_generate.as_deref(),
                    pgo_use.as_deref(),
                )?)
            }
        }
        Command::Build {
            path,
            output,
            profile,
            shared,
            node,
            march_native: _,
            explain,
            no_kiln_extensions,
            explain_parallel,
            static_link,
            pgo_generate,
            pgo_use,
        } => {
            let node = if shared { false } else { auto_node(node) };
            let mut backend = open(one_shot, &socket)?;
            let mut built = commands::build(
                backend.as_mut(),
                &path,
                &root,
                output.as_deref(),
                profile,
                shared,
                node,
                explain || explain_parallel,
                no_kiln_extensions,
                static_link,
                pgo_generate.as_deref(),
                pgo_use.as_deref(),
            )?;
            if explain_parallel && !explain {
                built.text = render::parallel_explanation_only(&built.text);
            }
            report(built)
        }
        Command::Check { path, node } => {
            let node = auto_node(node);
            let mut backend = open(one_shot, &socket)?;
            report(commands::check(backend.as_mut(), &path, &root, node)?)
        }
        Command::Test {
            path,
            changed,
            parse_only,
            node,
            output,
        } => {
            let mut backend = open(one_shot, &socket)?;
            report(commands::test(
                backend.as_mut(),
                &path,
                &root,
                changed,
                parse_only,
                node,
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
        Command::Watch {
            path,
            interval,
            node,
        } => {
            let node = auto_node(node);
            let mut backend = open(one_shot, &socket)?;
            let mut out = std::io::stdout().lock();
            let mut notes = std::io::stderr().lock();
            watch::run(
                backend.as_mut(),
                &path,
                &root,
                interval,
                node,
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
        Command::Exec { binary, args } => exec::run_exec(&binary, &args, &root),
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
    // The program's own standard error, before Kiln's notes and without the `kiln:` prefix: the
    // two streams are separate so that a caller redirecting one and not the other gets what the
    // program actually wrote, which is what `console.error` going to stderr is for.
    emit_error(&output.error_text)?;
    for note in &output.notes {
        if note.contains('\n') {
            eprintln!("{note}");
        } else {
            eprintln!("kiln: {note}");
        }
    }
    Ok(output.exit_code)
}

fn emit_error(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(text.as_bytes())
        .and_then(|()| stderr.flush())
        .map_err(|source| Error::io("cannot write to stderr", source))
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
            "x",
            "--one-shot",
            "--socket",
            "--interval",
            "--out",
            "--dry-run",
            "--march=native",
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
                "run", "build", "check", "test", "project", "stats", "watch", "daemon", "cache",
                "x"
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

    #[test]
    fn find_project_root_locates_ancestor_package_json() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-test-project-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        let pkg_dir = fixture.join("my-package");
        let sub_dir = pkg_dir.join("src/nested");
        std::fs::create_dir_all(&sub_dir).expect("create test directories");
        std::fs::write(pkg_dir.join("package.json"), "{}").expect("write package.json");
        let target_file = sub_dir.join("index.js");
        std::fs::write(&target_file, "console.log(1);").expect("write file");

        let found = find_project_root(Some(&target_file)).expect("discover root");
        assert_eq!(found, pkg_dir.canonicalize().expect("canonical pkg_dir"));

        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn find_project_root_locates_ancestor_git_dir() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-test-git-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        let repo_dir = fixture.join("repo");
        let sub_dir = repo_dir.join("scripts");
        std::fs::create_dir_all(&sub_dir).expect("create test directories");
        std::fs::create_dir_all(repo_dir.join(".git")).expect("create .git");
        let target_file = sub_dir.join("tool.js");
        std::fs::write(&target_file, "console.log(1);").expect("write file");

        let found = find_project_root(Some(&target_file)).expect("discover root");
        assert_eq!(found, repo_dir.canonicalize().expect("canonical repo_dir"));

        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn find_project_root_locates_ancestor_tsconfig_json() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-test-tsconfig-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        let project_dir = fixture.join("ts-project");
        let sub_dir = project_dir.join("src/nested");
        std::fs::create_dir_all(&sub_dir).expect("create test directories");
        std::fs::write(project_dir.join("tsconfig.json"), "{}").expect("write tsconfig.json");
        let target_file = sub_dir.join("app.ts");
        std::fs::write(&target_file, "console.log(1);").expect("write file");

        let found = find_project_root(Some(&target_file)).expect("discover root");
        assert_eq!(
            found,
            project_dir.canonicalize().expect("canonical project_dir")
        );

        let _ = std::fs::remove_dir_all(&fixture);
    }

    #[test]
    fn find_project_root_locates_monorepo_workspace_root() {
        let fixture =
            std::env::temp_dir().join(format!("kiln-test-monorepo-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fixture);
        let repo_dir = fixture.join("monorepo");
        let sub_dir = repo_dir.join("packages/subpkg/src");
        std::fs::create_dir_all(&sub_dir).expect("create test directories");
        std::fs::create_dir_all(repo_dir.join(".git")).expect("create .git");
        std::fs::write(
            repo_dir.join("package.json"),
            r#"{"workspaces": ["packages/*"]}"#,
        )
        .expect("write root package.json");
        std::fs::write(
            repo_dir.join("packages/subpkg/package.json"),
            r#"{"name": "subpkg"}"#,
        )
        .expect("write subpkg package.json");
        let target_file = sub_dir.join("index.ts");
        std::fs::write(&target_file, "console.log(1);").expect("write file");

        let found = find_project_root(Some(&target_file)).expect("discover root");
        assert_eq!(found, repo_dir.canonicalize().expect("canonical repo_dir"));

        let _ = std::fs::remove_dir_all(&fixture);
    }
}

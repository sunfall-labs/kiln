//! node as a third reference, and the reason two engines agreeing is not evidence.
//!
//! WHY THIS MODULE EXISTS. `engines.rs` compares the interpreter against the native backend. They
//! consume the same bytecode, from the same lowering, behind the same frontend, and they share
//! every decision either of those made. **When both are wrong in the same way the comparison sees
//! agreement and reports a pass** -- and that is not a hypothetical. `NOTES.md` records it twice
//! in this project's own words: *"Both engines were wrong in the same way, which is why no corpus
//! caught it: they agreed."* -- once for frozen arrays and once for `delete`. `parseInt({toString:
//! () => "99"})` is `99` in node and `NaN` in both engines as this is written.
//!
//! So this adds an implementation that shares nothing with either: `node`, off `PATH`. The
//! convention is `scripts/differential.sh`'s and is not reinvented here -- the reference answer is
//! *observed by running the program*, never transcribed, and node is run under `TZ=UTC` because
//! kiln's realm is always UTC (ADR 0015) and a timezone read from the environment is host state.
//!
//! THE OUTCOMES ARE KEPT APART ON PURPOSE. `both engines agree and node differs` is the class this
//! file was written for; `the interpreter and the native backend disagree` is the class that
//! already had a home; `one engine refused something outside its implemented surface` is a backlog
//! entry; `node itself refused the source` is a bug in the *generator*. Collapsing any two of
//! those into one bucket would be the fuzzer crying wolf, and a fuzzer that cries wolf gets
//! switched off.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one generated program may occupy node.
///
/// Generous, because it should never fire: the interpreter has already run the same program to
/// completion inside [`crate::engines::SCREENING_LIMITS`] before node is asked anything. If it
/// does fire, that is itself a finding.
pub const NODE_TIMEOUT: Duration = Duration::from_secs(10);

/// The same bound for a whole batch, which is [`BATCH`] programs rather than one.
pub const NODE_BATCH_TIMEOUT: Duration = Duration::from_secs(60);

/// How many programs go into one node process. See `node_batch.js` for why batching at all.
pub const BATCH: usize = 64;

/// A hard ceiling on node's heap, in megabytes.
///
/// The generator bounds allocation by construction (`js.rs` property 2: at most nine executions of
/// an innermost body, so `x = x + x` doubles a string nine times and not twenty-seven), and the
/// interpreter screen bounds it again. This is the third fence, and it is here because it is the
/// only one that survives a *bug* in either of the other two: a fuzzing loop that can be made to
/// allocate without bound eats the machine, and it is much cheaper to lose a case than a machine.
pub const NODE_HEAP_MB: u32 = 512;

/// A discovered `node`, plus the scratch directory it reads and writes through.
pub struct Node {
    program: PathBuf,
    version: String,
    workspace: PathBuf,
    driver: PathBuf,
}

impl Node {
    /// Find node and check it runs, or say why there is none.
    ///
    /// A missing node is a *skip* and never a failure, for the reason `scripts/differential.sh`
    /// gives: the reference implementation is the one dependency this check cannot fake, and
    /// pretending to have run it would be worse than saying it did not.
    pub fn discover() -> Result<Self, String> {
        let program =
            std::env::var_os("KILN_FUZZ_NODE").map_or_else(|| PathBuf::from("node"), PathBuf::from);
        let version = Command::new(&program)
            .arg("--version")
            .output()
            .map_err(|error| format!("cannot run {}: {error}", program.display()))?;
        if !version.status.success() {
            return Err(format!("{} --version failed", program.display()));
        }
        let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();

        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/reference");
        fs::create_dir_all(&workspace)
            .map_err(|error| format!("cannot create {}: {error}", workspace.display()))?;
        // Written out rather than referenced in place, so a `cargo install`ed or copied binary
        // still has its driver. It is a build-time constant, so this cannot go stale.
        let driver = workspace.join("node_batch.js");
        fs::write(&driver, include_str!("node_batch.js"))
            .map_err(|error| format!("cannot write {}: {error}", driver.display()))?;

        Ok(Self {
            program,
            version,
            workspace,
            driver,
        })
    }

    /// Which node, for a soak run's header line.
    pub fn describe(&self) -> String {
        format!("{} {}", self.program.display(), self.version)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            // kiln's realm is always UTC and node takes its zone from the environment. Without
            // this the two are not comparable at all. See ADR 0015 and `scripts/differential.sh`.
            .env("TZ", "UTC")
            .arg("--no-warnings")
            .arg(format!("--max-old-space-size={NODE_HEAP_MB}"))
            .stdin(Stdio::null());
        command
    }

    /// One generated program in its own node process, which is what
    /// `scripts/differential.sh` does and therefore the definition of the reference answer.
    ///
    /// Output goes to files rather than pipes, for the same reason `engines.rs` does it: a child
    /// that fills the pipe buffer while the parent waits for it to exit is a deadlock, and a
    /// fuzzing loop meets that eventually by construction.
    pub fn run(&self, source: &str, tag: &str) -> Result<NodeRun, String> {
        let directory = self.workspace.join(tag);
        fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let script = directory.join("case.js");
        fs::write(&script, source).map_err(|error| format!("cannot write a case: {error}"))?;
        let stdout_path = directory.join("stdout");
        let stderr_path = directory.join("stderr");

        let mut child = self
            .command()
            .arg(&script)
            .stdout(Stdio::from(create(&stdout_path)?))
            .stderr(Stdio::from(create(&stderr_path)?))
            .spawn()
            .map_err(|error| format!("cannot launch node: {error}"))?;
        let status = wait(&mut child, NODE_TIMEOUT)?;

        let stdout = read(&stdout_path);
        let stderr = read(&stderr_path);
        Ok(NodeRun {
            status: if status.success() {
                NodeStatus::Ok
            } else if stdout.is_empty() && stderr.contains("SyntaxError:") {
                // A SyntaxError before anything was printed never ran a line, so the generated
                // source was never JavaScript. That is a bug in `js.rs` and it is classified apart
                // from everything else for exactly that reason.
                NodeStatus::Invalid
            } else {
                NodeStatus::Threw
            },
            diagnostic: diagnostic(&stderr),
            stdout,
        })
    }

    /// Many generated programs in one node process, at roughly a thirtieth of the cost each.
    ///
    /// The answers are the same answers: `tests/node_reference.rs` pins this against [`Self::run`]
    /// byte for byte, because a faster reference that is not the reference is worse than no
    /// reference at all.
    ///
    /// A batch that cannot be completed -- node was killed, a program printed past the driver's
    /// output ceiling -- is re-run one process at a time rather than dropped, so a single
    /// pathological case never costs the sixty-three it was batched with.
    pub fn run_batch(&self, sources: &[String]) -> Result<Vec<NodeRun>, String> {
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        let input = self.workspace.join("batch-in");
        let output = self.workspace.join("batch-out");
        let _ = fs::remove_file(&output);

        let mut encoded = format!("{}\n", sources.len()).into_bytes();
        for source in sources {
            encoded.extend_from_slice(format!("{}\n", source.len()).as_bytes());
            encoded.extend_from_slice(source.as_bytes());
        }
        fs::write(&input, &encoded).map_err(|error| format!("cannot write a batch: {error}"))?;

        let mut child = self
            .command()
            .arg(&self.driver)
            .arg(&input)
            .arg(&output)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot launch node: {error}"))?;
        let status = wait(&mut child, NODE_BATCH_TIMEOUT);

        let decoded = match status {
            Ok(status) if status.success() => decode(&fs::read(&output).unwrap_or_default()),
            _ => None,
        };
        match decoded {
            Some(runs) if runs.len() == sources.len() => Ok(self.repair(sources, runs)),
            // The driver itself failing is not a result about any one program, so nothing is
            // concluded from it: every case in the batch is answered the slow, definitely-correct
            // way instead.
            _ => sources
                .iter()
                .enumerate()
                .map(|(index, source)| self.run(source, &format!("fallback-{index}")))
                .collect(),
        }
    }

    /// Re-run, one process each, the cases the driver could not answer.
    fn repair(&self, sources: &[String], mut runs: Vec<NodeRun>) -> Vec<NodeRun> {
        for (index, run) in runs.iter_mut().enumerate() {
            if run.status == NodeStatus::Unusable
                && let Ok(replacement) = self.run(&sources[index], &format!("repair-{index}"))
            {
                *run = replacement;
            }
        }
        runs
    }
}

/// How node ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Ran to the end and exited 0.
    Ok,
    /// Threw, which generated programs do on purpose: `Statement::Throw` exists.
    Threw,
    /// node refused the source. **A bug in the generator, not in kiln** -- the whole premise of
    /// `js.rs` is that every value of `JsProgram` renders to valid JavaScript, so this is that
    /// premise failing and it is reported as its own class.
    Invalid,
    /// The batch driver could not answer for this case and the fallback did not either.
    Unusable,
}

/// What node printed, and how it ended.
#[derive(Debug, Clone)]
pub struct NodeRun {
    pub stdout: String,
    /// The first line of whatever node said about stopping. Grouped, not shown per case.
    pub diagnostic: String,
    pub status: NodeStatus,
}

impl NodeRun {
    pub const fn completed(&self) -> bool {
        matches!(self.status, NodeStatus::Ok)
    }
}

fn decode(bytes: &[u8]) -> Option<Vec<NodeRun>> {
    let mut cursor = 0_usize;
    let line = |cursor: &mut usize| -> Option<String> {
        let end = bytes[*cursor..].iter().position(|byte| *byte == b'\n')? + *cursor;
        let text = String::from_utf8_lossy(&bytes[*cursor..end]).into_owned();
        *cursor = end + 1;
        Some(text)
    };
    let count: usize = line(&mut cursor)?.parse().ok()?;
    let mut runs = Vec::with_capacity(count);
    for _ in 0..count {
        let status = match line(&mut cursor)?.as_str() {
            "ok" => NodeStatus::Ok,
            "throw" => NodeStatus::Threw,
            "invalid" => NodeStatus::Invalid,
            _ => NodeStatus::Unusable,
        };
        let field = |cursor: &mut usize| -> Option<String> {
            let length: usize = line(cursor)?.parse().ok()?;
            let text = String::from_utf8_lossy(bytes.get(*cursor..*cursor + length)?).into_owned();
            *cursor += length;
            Some(text)
        };
        let diagnostic = field(&mut cursor)?;
        let stdout = field(&mut cursor)?;
        runs.push(NodeRun {
            stdout,
            diagnostic,
            status,
        });
    }
    Some(runs)
}

fn wait(
    child: &mut std::process::Child,
    limit: Duration,
) -> Result<std::process::ExitStatus, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => return Err(format!("waiting for node failed: {error}")),
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("node did not finish within {limit:?}"));
        }
        std::thread::sleep(Duration::from_micros(250));
    }
}

/// What node said about stopping, reduced to the one line worth grouping by.
///
/// node's stderr for an uncaught error opens with the offending file and line, a copy of the
/// source, and a caret -- all of it specific to the case and none of it a reason. The reason is
/// the `SomeError: message` line underneath, and finding it is what turns eight hundred instances
/// of one bug into one line of a report rather than eight hundred.
fn diagnostic(stderr: &str) -> String {
    let named_error = stderr.lines().find(|line| {
        line.split_once(':')
            .is_some_and(|(head, _)| head.ends_with("Error") && !head.contains(' '))
    });
    named_error
        .or_else(|| {
            // A `throw 1` has no error name at all. What node prints for it is preceded by
            // `Uncaught`-style framing in some versions and bare in others, so the fallback is the
            // first line that is not part of the code frame.
            stderr
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with("    at "))
        })
        .unwrap_or("(silence)")
        .trim()
        .to_owned()
}

fn create(path: &Path) -> Result<fs::File, String> {
    fs::File::create(path).map_err(|error| format!("cannot create {}: {error}", path.display()))
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(&fs::read(path).unwrap_or_default()).into_owned()
}

//! Running one program through both engines and classifying the result.
//!
//! The classification is `scripts/native.sh`'s, deliberately and unchanged, because that script
//! is where the project already decided what "the same output" means and a second definition
//! would be a second answer to the same question:
//!
//!   * **match** — both engines ran to completion and printed the same bytes.
//!   * **refused** — the C backend named an instruction it does not implement yet. The native
//!     surface is deliberately growing, so a refusal is a work queue entry, not a defect.
//!   * **DIVERGES** — both ran to completion and printed different bytes. One of them is wrong
//!     and there is nothing else it could be: they consume the *same bytecode*, so no
//!     specification reading, node version, or locale enters into it.
//!
//! Two outcomes exist here that the shell script has no need for. `InvalidC` is a generated
//! translation unit the platform compiler rejected — the shell script never sees it because its
//! corpus is hand-written and already compiles, but a fuzzer will, and it is a real defect in
//! the emitter rather than a refusal. `TooExpensive` is the guard described on
//! [`SCREENING_LIMITS`].

use std::cell::OnceCell;
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use kiln_artifact_cache::ArtifactCache;
use kiln_hir::SourceGoal;
use kiln_interpreter::{ExecutionLimits, execute_reclaiming};
use kiln_ir::Program;
use kiln_native::{NativeBuildOptions, NativeBuilder, NativeError, NativeProfile};
use kiln_native_runtime::NATIVE_RESULT_PATH_ENV;
use kiln_native_toolchain::Toolchain;
use kiln_runtime::CaptureHost;

/// The interpreter's screening budget, and the reason the native side is safe to run unattended.
///
/// The two engines run the same bytecode, so a program the interpreter finishes in a few
/// thousand steps is a program the native binary finishes quickly too. Screening on the cheap
/// engine first therefore bounds the expensive one, which matters because a native binary has no
/// step limit, no supervisor, and — if a generated program allocated without bound — no ceiling
/// short of the machine's. This is a hard cap by design: see the loop and nesting bounds in
/// `js.rs`, which exist for the same reason.
///
/// It is far below [`ExecutionLimits::default`] on purpose. A fuzz corpus wants many cheap
/// programs, not a few expensive ones, and a program that needs a million steps to disagree with
/// itself almost certainly has a thousand-step cousin that does too.
pub const SCREENING_LIMITS: ExecutionLimits = ExecutionLimits {
    max_steps: 50_000,
    max_call_depth: 64,
    max_native_depth: 32,
};

/// Whether a program the interpreter could not finish is still compared.
///
/// A generated `throw` is common, and the bytes printed *before* it are as much an observation
/// as any other. The risk is the reverse: if the native runtime were to abort without flushing
/// what it had already written, every throwing program would look like a divergence and the
/// signal would be buried. Measured before this was turned on -- see `fuzz/README.md`.
pub const COMPARE_THROWING_PROGRAMS: bool = true;

/// How long a native binary may run before the harness kills it.
///
/// Belt and braces over the screening limit above. If this ever fires it is itself a finding:
/// the interpreter finished the same bytecode inside 50k steps.
pub const NATIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// What happened when one program was put through both engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The frontend or lowering refused the source. Neither engine was asked anything.
    NotLowered(String),
    /// The interpreter did not run the program to completion — a thrown error, an unresolved
    /// identifier (ADR 0017), or the screening budget. Nothing to compare against.
    InterpreterStopped(String),
    /// The program cost more than [`SCREENING_LIMITS`] allows, so the native side was not run.
    TooExpensive,
    /// The C backend refused the bytecode and said which instruction. The backlog, not a defect.
    NativeRefused(String),
    /// The native binary refused *at run time*: it exited non-zero with a diagnostic on stderr,
    /// having printed a prefix of what the interpreter printed.
    ///
    /// This is a refusal and not a divergence, and the distinction is the same one
    /// `scripts/native.sh` draws. The emitted runtime returns `KILN_C99_CONTROL_ERROR` wherever
    /// it meets something outside its implemented surface -- a property read whose key is not in
    /// its method table, for instance -- and the launcher prints why and exits 1. Nobody is
    /// misled by that. It is a work queue entry that names itself, and if it were counted as a
    /// divergence it would bury the outcome that actually matters underneath itself.
    ///
    /// The prefix check is what keeps this honest: bytes printed *before* the refusal are still
    /// an answer, and if they differ this is reported as a divergence instead.
    NativeStoppedAtRuntime(String),
    /// Both engines refused, and agreed on everything they printed first.
    BothStopped(String),
    /// No C toolchain, or the harness itself could not proceed.
    Unavailable(String),
    /// Both engines ran to completion and printed the same bytes.
    Match,

    // --- Everything below this line is a bug in kiln. ---
    /// The emitter produced a translation unit the platform compiler or linker rejected.
    InvalidC(String),
    /// The native binary died of a signal, or outlived [`NATIVE_TIMEOUT`].
    NativeCrashed(String),
    /// Both engines ran the same bytecode and printed different bytes.
    Diverged {
        source: String,
        interpreter: String,
        native: String,
        /// Carried so a report can be acted on without re-running: an empty native stdout with
        /// a non-zero exit and a diagnostic on stderr is a different bug from an empty native
        /// stdout with a clean exit.
        native_exit: Option<i32>,
        native_stderr: String,
    },
}

impl Outcome {
    /// Whether this outcome is a defect. The whole point of the harness is that this is a small,
    /// closed set and that everything else is either progress or a deliberate gap.
    pub const fn is_defect(&self) -> bool {
        matches!(
            self,
            Self::InvalidC(_) | Self::NativeCrashed(_) | Self::Diverged { .. }
        )
    }

    /// The diagnostic behind a non-defect outcome, normalised enough to group by.
    ///
    /// Grouping is the point: a hundred distinct spellings of "unsupported statement: class
    /// declaration" is a list, and one line saying it happened a hundred times is a backlog.
    pub fn reason(&self) -> Option<String> {
        let raw = match self {
            Self::NotLowered(reason)
            | Self::InterpreterStopped(reason)
            | Self::NativeRefused(reason)
            | Self::NativeStoppedAtRuntime(reason)
            | Self::BothStopped(reason)
            | Self::Unavailable(reason) => reason,
            Self::Match
            | Self::TooExpensive
            | Self::InvalidC(_)
            | Self::NativeCrashed(_)
            | Self::Diverged { .. } => {
                return None;
            }
        };
        // Everything after a backtick-quoted fragment is the specific name, not the class of
        // failure. Truncating there is what turns per-identifier noise into a countable reason.
        Some(match raw.find('`') {
            Some(index) => raw[..index].trim_end().to_owned(),
            None => raw.clone(),
        })
    }

    /// A short, stable name for tallying a soak run.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::NotLowered(_) => "not-lowered",
            Self::InterpreterStopped(_) => "interpreter-stopped",
            Self::TooExpensive => "too-expensive",
            Self::NativeRefused(_) => "native-refused",
            Self::NativeStoppedAtRuntime(_) => "native-stopped",
            Self::BothStopped(_) => "both-stopped",
            Self::Unavailable(_) => "unavailable",
            Self::Match => "match",
            Self::InvalidC(_) => "INVALID-C",
            Self::NativeCrashed(_) => "NATIVE-CRASHED",
            Self::Diverged { .. } => "DIVERGED",
        }
    }
}

/// Source text through the real frontend and the real lowering, to the same `Program` both
/// engines consume.
///
/// Using `kiln_ir::compile` rather than building a `Program` by hand is what makes the two
/// engines comparable at all: the invariant under test is that they agree on *the same
/// bytecode*, and bytecode the compiler would never emit is not evidence about that.
pub fn lower(source: &str) -> Result<Program, String> {
    let module = frontend_oxc::parse_with_goal("fuzz.js", source, SourceGoal::Script);
    if module.panicked {
        return Err("frontend parser could not recover a program".to_owned());
    }
    if let Some(diagnostic) = module.diagnostics.first() {
        return Err(format!("parse: {}", diagnostic.message));
    }
    kiln_ir::compile(&module).map_err(|diagnostics| {
        diagnostics.first().map_or_else(
            || "lowering refused the program without saying why".to_owned(),
            |diagnostic| format!("compile: {}", diagnostic.message),
        )
    })
}

/// Result of the cheap engine: the bytes it printed, and whether it got to the end.
pub enum Interpreted {
    Completed(String),
    /// Threw, or hit a limit other than the step budget. `stdout` is what it had printed by
    /// then, which is still comparable: an engine that prints three lines before throwing and
    /// one that prints two have disagreed regardless of how either of them ended.
    Stopped {
        reason: String,
        stdout: String,
    },
    OverBudget,
}

/// Run the interpreter.
///
/// Nothing `Rc`-bearing escapes: the `Rc<Program>`, the VM, and every `Value` it allocates are
/// created and dropped inside this function, and only a `String` comes back. `kiln-interpreter`
/// is `!Send` (ADR 0007's object model is `Rc`-based throughout), so a harness that let a value
/// outlive this frame would not compile the moment anything wanted to move it — this keeps the
/// question from ever arising.
///
/// `execute_reclaiming` rather than `execute` because a fuzz loop is exactly the embedder its
/// doc comment describes: many independent programs in one process, where `execute` would leave
/// every function of every program allocated for the life of the run.
pub fn interpret(program: &Program) -> Interpreted {
    let mut host = CaptureHost::new();
    match execute_reclaiming(Rc::new(program.clone()), &mut host, SCREENING_LIMITS) {
        Ok(_) => Interpreted::Completed(host.into_stdout()),
        Err(kiln_interpreter::ExecutionError::StepLimitExceeded { .. }) => Interpreted::OverBudget,
        Err(error) => Interpreted::Stopped {
            reason: error.to_string(),
            stdout: host.into_stdout(),
        },
    }
}

/// A discovered C toolchain plus the artifact cache it writes through.
///
/// One of these is built per process and reused, because `Toolchain::discover` shells out to
/// probe the compiler and the cache is what turns a repeated build into a link. Neither is
/// per-iteration work, and treating them as such is most of the difference between a native
/// iteration costing a second and costing three.
pub struct Native {
    builder: NativeBuilder,
    workspace: PathBuf,
    profile: NativeProfile,
}

impl Native {
    /// Discover a toolchain, or say why there is none.
    ///
    /// `NativeProfile::Dev` deliberately: this harness is looking for semantic disagreement
    /// between two implementations of the same bytecode, and the optimiser is not part of that
    /// question. It is also roughly twice as fast to compile, which at one C compilation per
    /// iteration is the whole budget.
    pub fn discover() -> Result<Self, String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/differential");
        let workspace = root.join("work");
        std::fs::create_dir_all(&workspace)
            .map_err(|error| format!("cannot create {}: {error}", workspace.display()))?;
        // `KILN_FUZZ_CC` overrides discovery, because which compiler is found decides most of
        // this target's cost and the default is not always the fast one. Measured figures for
        // both are in `fuzz/README.md`.
        let cache = ArtifactCache::new(root.join("cache"));
        let builder = match std::env::var_os("KILN_FUZZ_CC") {
            Some(path) => NativeBuilder::new(
                cache,
                Toolchain::from_compiler(&path)
                    .map_err(|error| format!("KILN_FUZZ_CC={path:?} is unusable: {error}"))?,
            ),
            None => NativeBuilder::discover_cached(root.join("cache"))
                .map_err(|error| format!("no usable C toolchain: {error}"))?,
        };
        Ok(Self {
            builder,
            workspace,
            profile: NativeProfile::Dev,
        })
    }

    /// Which compiler was found, for a soak run's header line.
    pub fn describe(&self) -> String {
        let identity = self.builder.toolchain().identity();
        format!("{:?} at {}", identity.kind, identity.path.display())
    }

    /// Build and run one program natively, capturing stdout.
    ///
    /// The executable is launched here rather than through `NativeBuilder::run` for one reason:
    /// that method uses `Command::output()`, which waits forever. A generated program is not a
    /// program anybody reviewed, so the harness needs to be able to give up on it. Output is
    /// redirected to files rather than pipes, which also removes the classic deadlock where a
    /// child fills the pipe buffer while the parent is blocked waiting for it to exit.
    fn run(&self, program: &Program, tag: &str) -> Result<NativeRun, Outcome> {
        let directory = self.workspace.join(tag);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).map_err(|error| {
            Outcome::Unavailable(format!("cannot create work directory: {error}"))
        })?;
        let executable = directory.join("program");

        let report = self
            .builder
            .build(
                program,
                &NativeBuildOptions {
                    output: executable.clone(),
                    profile: self.profile,
                },
            )
            .map_err(|error| classify_build_error(&error))?;

        let stdout_path = directory.join("stdout");
        let stderr_path = directory.join("stderr");
        let mut child = Command::new(&executable)
            .env(NATIVE_RESULT_PATH_ENV, directory.join("result.bin"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(create(&stdout_path)?))
            .stderr(Stdio::from(create(&stderr_path)?))
            .spawn()
            .map_err(|error| {
                Outcome::NativeCrashed(format!("cannot launch {}: {error}", executable.display()))
            })?;

        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    return Err(Outcome::NativeCrashed(format!("wait failed: {error}")));
                }
            }
            if started.elapsed() > NATIVE_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Outcome::NativeCrashed(format!(
                    "native binary did not finish within {NATIVE_TIMEOUT:?}, but the interpreter \
                     ran the same bytecode inside {} steps",
                    SCREENING_LIMITS.max_steps
                )));
            }
            // Fine-grained, because the thing being waited on usually finishes in single-digit
            // milliseconds. The first execution of a freshly written binary is far slower than
            // the second -- around 180 ms here, which is macOS validating a signature it has
            // never seen -- and that cost is inherent to compiling a new program every
            // iteration rather than something the poll interval can recover.
            std::thread::sleep(Duration::from_micros(250));
        };

        Ok(NativeRun {
            stdout: read(&stdout_path),
            stderr: read(&stderr_path),
            exit_code: status.code(),
            timings: report.timings,
            launch: started.elapsed(),
            cached: report.cache_hits.executable,
        })
    }
}

/// One discovered toolchain per thread, reached without capturing it.
///
/// `NativeBuilder` holds an `Arc<dyn ProcessRunner>`, and a trait object may contain interior
/// mutability, so a closure that captures one is not `RefUnwindSafe` -- which every bolero
/// engine requires, because it runs the closure inside `catch_unwind`. Reaching the toolchain
/// through a thread-local instead of capturing it satisfies that without an `AssertUnwindSafe`
/// that would be asserting something nobody checked.
///
/// Thread-local rather than a `static` for the same reason the rest of this crate is careful
/// about threads: the value stays on the thread that made it, so nothing here can put a
/// `!Send` neighbour in a position to travel.
pub fn with_native<R>(action: impl FnOnce(Result<&Native, &str>) -> R) -> R {
    thread_local! {
        static NATIVE: OnceCell<Result<Native, String>> = const { OnceCell::new() };
    }
    NATIVE.with(|cell| {
        let held = cell.get_or_init(Native::discover);
        action(held.as_ref().map_err(String::as_str))
    })
}

pub struct NativeRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timings: kiln_native::BuildTimings,
    pub launch: Duration,
    pub cached: bool,
}

/// Where a native iteration's time went, accumulated over a run.
///
/// Reported rather than guessed, because "the native target is slow" is not an actionable
/// statement and "of eight hundred milliseconds, seven hundred are the platform C compiler" is.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cost {
    pub cases: u32,
    pub emit: Duration,
    pub compile: Duration,
    pub link: Duration,
    pub build: Duration,
    pub launch: Duration,
}

impl Cost {
    pub fn observe(&mut self, run: &NativeRun) {
        self.cases += 1;
        self.emit += run.timings.emit;
        self.compile += run.timings.compile;
        self.link += run.timings.link;
        self.build += run.timings.total;
        self.launch += run.launch;
    }

    pub fn report(&self) -> String {
        if self.cases == 0 {
            return "no native case ran".to_owned();
        }
        let each = |total: Duration| total / self.cases;
        format!(
            "per native case: emit {:.1?}, C compile {:.1?}, link {:.1?}, whole build {:.1?},              launch {:.1?}",
            each(self.emit),
            each(self.compile),
            each(self.link),
            each(self.build),
            each(self.launch)
        )
    }
}

/// An emitter refusal is the growing native surface; anything the platform compiler said is a
/// generated translation unit that does not compile, which is a defect in the emitter.
fn classify_build_error(error: &NativeError) -> Outcome {
    match error {
        NativeError::Emit(kiln_backend_c::EmitError::Unsupported { feature }) => {
            Outcome::NativeRefused(feature.clone())
        }
        NativeError::Emit(kiln_backend_c::EmitError::InvalidProgram { message }) => {
            // The emitter's own validator rejecting bytecode the compiler produced is a
            // disagreement between two parts of kiln, not a missing feature.
            Outcome::InvalidC(format!("emitter rejected compiler output: {message}"))
        }
        NativeError::Toolchain(toolchain) => Outcome::InvalidC(toolchain.to_string()),
        other => Outcome::Unavailable(other.to_string()),
    }
}

fn create(path: &Path) -> Result<File, Outcome> {
    File::create(path)
        .map_err(|error| Outcome::Unavailable(format!("cannot create {}: {error}", path.display())))
}

fn read(path: &Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The whole comparison, from source text to a classified outcome.
///
/// `tag` names the per-iteration work directory. Callers running more than one of these at once
/// must pass distinct tags.
pub fn compare(native: &Native, source: &str, tag: &str) -> Outcome {
    compare_recording(native, source, tag, &mut Cost::default())
}

/// [`compare`], with the per-iteration cost accumulated into `cost`.
pub fn compare_recording(native: &Native, source: &str, tag: &str, cost: &mut Cost) -> Outcome {
    let program = match lower(source) {
        Ok(program) => program,
        Err(reason) => return Outcome::NotLowered(reason),
    };

    // The cheap engine first, and not only because it is cheap: its verdict is what makes the
    // expensive one safe to run. See SCREENING_LIMITS.
    let (expected, threw) = match interpret(&program) {
        Interpreted::Completed(stdout) => (stdout, false),
        Interpreted::Stopped { reason, stdout } => {
            if !COMPARE_THROWING_PROGRAMS {
                return Outcome::InterpreterStopped(reason);
            }
            (stdout, true)
        }
        Interpreted::OverBudget => return Outcome::TooExpensive,
    };

    let run = match native.run(&program, tag) {
        Ok(run) => run,
        Err(outcome) => return outcome,
    };
    cost.observe(&run);

    // A signal has no exit code. That is a crash, and it is a defect even when stdout happens to
    // agree up to the point the process died.
    if run.exit_code.is_none() {
        return Outcome::NativeCrashed(format!(
            "killed by a signal; stderr: {}",
            excerpt(&run.stderr)
        ));
    }
    // The four cases, and the reason each is judged the way it is.
    //
    // The rule underneath all of them: an engine that STOPS is allowed to have printed less, and
    // an engine that FINISHES is not allowed to have printed anything different. That is what
    // makes a growing native surface reportable as a backlog without giving up the ability to
    // see a wrong answer inside the bytes it did produce.
    let stopped = run.exit_code != Some(0);
    match (threw, stopped) {
        // Both ran to the end. Byte equality or nothing; this is the case the project's stated
        // worst failure mode lives in.
        (false, false) => {
            if run.stdout == expected {
                Outcome::Match
            } else {
                diverged(source, expected, run)
            }
        }
        // The interpreter finished and the native binary gave up part way. A prefix is a
        // refusal; anything else is a wrong answer wearing a refusal's clothes.
        (false, true) => {
            if expected.starts_with(&run.stdout) {
                Outcome::NativeStoppedAtRuntime(native_reason(&run.stderr))
            } else {
                diverged(source, expected, run)
            }
        }
        // The interpreter refused and the native binary ran to completion, which means native
        // produced an answer the interpreter says does not exist. Always a defect: a compiler
        // may not have two opinions about whether a program has a value.
        (true, false) => Outcome::Diverged {
            source: source.to_owned(),
            interpreter: format!("{expected}<the interpreter stopped here>"),
            native: run.stdout,
            native_exit: run.exit_code,
            native_stderr: run.stderr,
        },
        // Both refused. They may have stopped at different points, so neither side owns the
        // longer output; requiring one to prefix the other is the strongest claim the pair
        // supports.
        (true, true) => {
            if expected.starts_with(&run.stdout) || run.stdout.starts_with(&expected) {
                Outcome::BothStopped(native_reason(&run.stderr))
            } else {
                diverged(source, expected, run)
            }
        }
    }
}

fn diverged(source: &str, expected: String, run: NativeRun) -> Outcome {
    Outcome::Diverged {
        source: source.to_owned(),
        interpreter: expected,
        native: run.stdout,
        native_exit: run.exit_code,
        native_stderr: run.stderr,
    }
}

/// The native runtime's own account of why it stopped, reduced to something countable.
///
/// The emitted launcher prints either `uncaught <value>` for a thrown exception or
/// `the program stopped with status <n>` for an internal refusal. The thrown value is
/// program-specific and the status is not, so only the status survives grouping.
fn native_reason(stderr: &str) -> String {
    let first = stderr.lines().next().unwrap_or("(silence)");
    if first.starts_with("uncaught") {
        return "uncaught exception".to_owned();
    }
    first.to_owned()
}

fn excerpt(text: &str) -> String {
    let mut end = 200.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{:?}", &text[..end])
}

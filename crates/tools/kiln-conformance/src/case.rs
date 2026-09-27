//! The vocabulary every conformance run reports in.
//!
//! Kiln has two ways of learning what a test expects: Test262 frontmatter inside the file, and an
//! external manifest beside a corpus that was copied from elsewhere and must stay byte-identical.
//! Both produce the types below, so nothing downstream of a run has to know which was used.

use std::time::{Duration, Instant};

use kiln_hir::Span;

/// Which part of the compiler a conformance case exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunMode {
    /// Parse and lower enough to observe parser diagnostics, but do not compile or execute.
    Parse,
    /// Parse, compile to Kiln bytecode, and execute in a fresh interpreter/host pair.
    Interpreter,
    /// Execute through the interpreter with its Tokio-owned Node host Adapter installed.
    Node,
}

/// The realm variant used for an invocation.
///
/// A plain Test262 test is tested in both sloppy and strict variants.  The metadata flags select
/// one of those variants, while module tests have their own (implicitly strict) parse goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variant {
    Sloppy,
    Strict,
    Module,
}

impl Variant {
    /// The name this variant is reported under, and the suffix of a variant-qualified test id.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sloppy => "sloppy",
            Self::Strict => "strict",
            Self::Module => "module",
        }
    }
}

/// The classification of a single conformance case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Outcome {
    /// The observed behavior agrees with the case's expectation.
    Pass,
    /// The case was applicable, but the observed behavior disagreed with its expectation.
    Fail,
    /// The case is valid but requires a language/runtime capability this runner does not have.
    Unsupported,
    /// The case was deliberately not run (for example, it needs a harness or a build step).
    Skipped,
}

impl Outcome {
    /// A short, stable tag for grepping and grouping.  Deliberately not `Debug`: these strings
    /// end up in reports people write scripts against, so they may not drift with a rename.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unsupported => "unsupported",
            Self::Skipped => "skipped",
        }
    }
}

/// The compiler phase that produced a runner diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DiagnosticPhase {
    /// The runner itself declined, before any of the test's own source was considered.
    Harness,
    Parse,
    Compile,
    Runtime,
    /// The program ran, and what it produced disagreed with what the corpus recorded.
    Expectation,
}

impl DiagnosticPhase {
    /// A short, stable tag; see [`Outcome::tag`] for why this is not derived from `Debug`.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Harness => "harness",
            Self::Parse => "parse",
            Self::Compile => "compile",
            Self::Runtime => "runtime",
            Self::Expectation => "expectation",
        }
    }
}

/// A diagnostic retained in a [`CaseResult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: DiagnosticPhase,
    pub message: String,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub(crate) fn new(
        phase: DiagnosticPhase,
        message: impl Into<String>,
        span: Option<Span>,
    ) -> Self {
        Self {
            phase,
            message: message.into(),
            span,
        }
    }
}

/// Timing information for one case.  Keeping this as a value rather than printing inside the
/// runner lets a coordinator aggregate percentiles without coupling this crate to a reporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub elapsed: Duration,
}

/// The result of running one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseResult {
    pub outcome: Outcome,
    pub diagnostics: Vec<Diagnostic>,
    pub timing: Timing,
    /// Captured output from a successful or failed interpreter invocation.
    pub stdout: String,
}

impl CaseResult {
    pub(crate) fn new(
        outcome: Outcome,
        diagnostics: Vec<Diagnostic>,
        stdout: String,
        started: Instant,
    ) -> Self {
        Self {
            outcome,
            diagnostics,
            timing: Timing {
                elapsed: started.elapsed(),
            },
            stdout,
        }
    }

    /// The diagnostic that explains the outcome, which is the first one recorded.
    ///
    /// Runners push the decisive diagnostic first precisely so that a report which has room for
    /// one line per case shows the reason rather than an incidental warning.
    pub fn primary_diagnostic(&self) -> Option<&Diagnostic> {
        self.diagnostics.first()
    }
}

/// Alias used by callers that refer to each invocation as a run rather than a case.
pub type RunResult = CaseResult;

/// A case result paired with the strictness/parse-goal variant that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantResult {
    pub variant: Variant,
    pub result: CaseResult,
}

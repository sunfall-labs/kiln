//! Running Kiln's conformance corpora and reporting what actually happened.
//!
//! A test reaches this crate one of two ways.  Most declare themselves in Test262-style
//! `/*--- … ---*/` frontmatter, which stays the convention for tests Kiln writes itself and is the
//! only source that can state a negative-test or strictness contract.  The rest are declared by a
//! `manifest.toml` beside the corpus, which exists for `conformance/`: those files were copied
//! byte for byte from the project they were proven in, and one probe is a single line whose entire
//! point is that it contains nothing else, so their metadata cannot live inside them.
//!
//! [`discover`] picks the source and returns [`ConformanceTest`] either way, so selection, running
//! and reporting are indifferent to which was used.
//!
//! Nothing here has a process-global VM or host: every run creates a new host and asks the
//! interpreter for a new VM, which is the same execution path as the CLI's development mode.
//!
//! [`Manifest`] is public separately from discovery because a manifest is a repository artifact
//! in its own right — an inventory of a corpus with its provenance — and reading one without
//! running it is a reasonable thing to want.
//!
//! ## Reading a result honestly
//!
//! [`Outcome::Skipped`] is not [`Outcome::Pass`].  A corpus can be entirely skipped — the Effect
//! programs need an out-of-process bundle step — and a report that folds the two together says a
//! suite passed when the suite did not run.  Every reporting path must keep them apart, and
//! [`ReasonReport`] is how the reasons stay visible instead of collapsing to a boolean.

mod case;
mod discovery;
mod harness;
mod manifest;
pub mod node_compat;
mod program;
mod reasons;
mod source_compiler;
mod test262;
mod toml_lite;

pub use case::{
    CaseResult, Diagnostic, DiagnosticPhase, Outcome, RunMode, RunResult, Timing, Variant,
    VariantResult,
};
pub use discovery::{ConformanceTest, DiscoveryError, Provenance, discover};
pub use harness::{
    ASYNC_COMPLETE, ASYNC_FAILURE, ASYNC_INCLUDE, DEFAULT_INCLUDES, HOST_CAPABILITY_MARKER,
    HOST_OBJECT_PRELUDE, Harness,
};
pub use manifest::{MANIFEST_FILE_NAME, Manifest, ManifestError, ManifestTest};
pub use program::run_node_against_expected_stdout;
pub use reasons::{ReasonGroup, ReasonReport};
pub use test262::{
    run_case, run_case_variants, run_case_variants_with_harness, run_case_with_harness,
    run_variant, run_variant_with_harness,
};

//! Differential executor comparing JavaScript programs across Node, Kiln interpreter,
//! and Kiln native under KILN_SPECULATE=0, 1, and exit.

use crate::engines::{self, Interpreted, Native, Outcome};
use crate::node::Node;

#[derive(Debug)]
pub enum SpeculateVerdict {
    Match,
    Refused(String),
    NotLowered(String),
    Diverged {
        source: String,
        node: String,
        interp: String,
        spec0: String,
        spec1: String,
        specexit: String,
        divergence_reason: String,
    },
    Crashed(String),
}

impl SpeculateVerdict {
    pub fn is_divergence(&self) -> bool {
        matches!(self, Self::Diverged { .. } | Self::Crashed(_))
    }
}

/// Run one program across Node, Interpreter, and Native under KILN_SPECULATE=0, 1, and exit.
pub fn compare_all_modes(
    native: &Native,
    node: &Node,
    source: &str,
    tag: &str,
) -> SpeculateVerdict {
    let program = match engines::lower(source) {
        Ok(p) => p,
        Err(err) => return SpeculateVerdict::NotLowered(err),
    };

    // 1. Node reference run
    let node_run = match node.run(source, tag) {
        Ok(run) => run,
        Err(err) => return SpeculateVerdict::Refused(format!("node failed: {err}")),
    };
    if !node_run.completed() {
        return SpeculateVerdict::Refused(format!("node did not complete: {:?}", node_run.status));
    }
    let node_out = node_run.stdout;

    // 2. Kiln interpreter run
    let interp_out = match engines::interpret(&program) {
        Interpreted::Completed(out) => out,
        Interpreted::Stopped { stdout, .. } => {
            return SpeculateVerdict::Refused(format!("interpreter stopped with prefix: {stdout}"));
        }
        Interpreted::OverBudget => {
            return SpeculateVerdict::Refused("interpreter exceeded step budget".into());
        }
    };

    // 3. Kiln native with KILN_SPECULATE=0
    let run_spec0 = match native.run_with_speculate(&program, tag, "0") {
        Ok(run) => run,
        Err(outcome) => return classify_outcome(outcome, "spec0"),
    };

    // 4. Kiln native with KILN_SPECULATE=1 (speculation enabled)
    let run_spec1 = match native.run_with_speculate(&program, tag, "1") {
        Ok(run) => run,
        Err(outcome) => return classify_outcome(outcome, "spec1"),
    };

    // 5. Kiln native with KILN_SPECULATE=exit (forced guard exits)
    let run_specexit = match native.run_with_speculate(&program, tag, "exit") {
        Ok(run) => run,
        Err(outcome) => return classify_outcome(outcome, "specexit"),
    };

    if run_spec0.exit_code != Some(0)
        || run_spec1.exit_code != Some(0)
        || run_specexit.exit_code != Some(0)
    {
        return SpeculateVerdict::Crashed(format!(
            "non-zero exit code: spec0={:?}, spec1={:?}, specexit={:?}",
            run_spec0.exit_code, run_spec1.exit_code, run_specexit.exit_code
        ));
    }

    let s0 = run_spec0.stdout;
    let s1 = run_spec1.stdout;
    let se = run_specexit.stdout;

    // Compare all outputs
    let mut divergences = Vec::new();
    if s0 != node_out {
        divergences.push("KILN_SPECULATE=0 differs from node");
    }
    if s1 != node_out {
        divergences.push("KILN_SPECULATE=1 differs from node");
    }
    if se != node_out {
        divergences.push("KILN_SPECULATE=exit differs from node");
    }
    if interp_out != node_out {
        divergences.push("interpreter differs from node");
    }
    if s0 != s1 {
        divergences.push("KILN_SPECULATE=0 differs from KILN_SPECULATE=1");
    }
    if s1 != se {
        divergences.push("KILN_SPECULATE=1 differs from KILN_SPECULATE=exit");
    }

    if divergences.is_empty() {
        SpeculateVerdict::Match
    } else {
        SpeculateVerdict::Diverged {
            source: source.to_string(),
            node: node_out,
            interp: interp_out,
            spec0: s0,
            spec1: s1,
            specexit: se,
            divergence_reason: divergences.join("; "),
        }
    }
}

fn classify_outcome(outcome: Outcome, mode: &str) -> SpeculateVerdict {
    match outcome {
        Outcome::NotLowered(r) => SpeculateVerdict::NotLowered(format!("{mode}: {r}")),
        Outcome::NativeRefused(r)
        | Outcome::NativeStoppedAtRuntime(r)
        | Outcome::BothStopped(r) => SpeculateVerdict::Refused(format!("{mode}: {r}")),
        Outcome::InvalidC(r) | Outcome::NativeCrashed(r) => {
            SpeculateVerdict::Crashed(format!("{mode}: {r}"))
        }
        _ => SpeculateVerdict::Refused(format!("{mode}: {outcome:?}")),
    }
}

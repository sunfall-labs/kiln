//! Three-way classification: node, the interpreter, and the native backend.
//!
//! `engines.rs` answers *do kiln's two engines agree with each other*. This answers *and are they
//! right* -- which is a different question, and the reason it is a different question is that the
//! two engines share a frontend, a lowering, and a set of builtins. Two implementations that share
//! their premises agree about their premises. See `node.rs`.
//!
//! THE COMPARISON RULE IS `engines.rs`'S, EXTENDED TO A THIRD PARTY, and it is stated once here
//! because getting it wrong in either direction is what makes a fuzzer useless:
//!
//!   **An engine that stops is allowed to have printed less. An engine that finishes is not
//!   allowed to have printed anything different.**
//!
//! So a kiln engine that refused halfway through a program node ran to the end is a *backlog
//! entry*, as long as what it did print is a prefix of what node printed. A kiln engine that ran
//! to the end and printed different bytes is a wrong answer. And a kiln engine that ran to the end
//! where *node* threw is also a wrong answer -- it produced a value node says the program does not
//! have, and `({valueOf(){return{}}, toString(){return{}}}) + 1` is exactly that shape.

use crate::engines::Interpreted;
use crate::node::{NodeRun, NodeStatus};

/// What one program's three answers add up to.
///
/// Ordered roughly by how much attention each deserves, and kept apart rather than bucketed: the
/// task of this crate is to make `both engines agree with each other but disagree with node` a
/// *reported* outcome, and it cannot be that if it shares a bucket with a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// node refused the source itself. The generator emitted something that is not JavaScript;
    /// nothing has been learned about kiln. Not a defect *of kiln*, and a defect all the same --
    /// see [`Class::is_generator_bug`].
    GeneratorBug,
    /// No comparison was possible: lowering refused the program, or the interpreter's screening
    /// budget stopped it before it produced an answer.
    NotComparable,
    /// A kiln engine stopped inside its implemented surface, having printed a prefix of what node
    /// printed. The growing native surface, and ADR 0017's unresolved identifiers. Backlog.
    Refused,
    /// Everything that ran printed the same bytes.
    Agrees,

    // --- Everything below this line is a wrong answer. ---
    /// The interpreter alone disagreed with node; the native backend agreed with node, or was not
    /// built. The class `Math.abs({valueOf: () => 42})` is in today.
    InterpreterDiffersFromNode,
    /// The native backend alone disagreed with node; the interpreter agreed with node. This is
    /// also an interpreter/native divergence, and is reported this way because node saying *which*
    /// engine is wrong is strictly more than the two-engine comparison could say.
    NativeDiffersFromNode,
    /// The two engines ran to completion, printed different bytes, and node matched neither. Two
    /// wrong answers and a third opinion.
    EnginesDivergeBothWrong,
    /// **The class this whole file exists for.** Both engines printed the same bytes and node
    /// printed different ones. Under the two-engine comparison this is a silent pass: they agree,
    /// so nothing is reported. `NOTES.md` records this happening twice already -- for frozen
    /// arrays and for `delete` -- in the project's own words: *"Both engines were wrong in the
    /// same way, which is why no corpus caught it: they agreed."*
    BothEnginesDifferFromNode,
}

impl Class {
    pub const fn label(self) -> &'static str {
        match self {
            Self::GeneratorBug => "GENERATOR-BUG",
            Self::NotComparable => "not-comparable",
            Self::Refused => "refused",
            Self::Agrees => "agrees",
            Self::InterpreterDiffersFromNode => "INTERPRETER-vs-NODE",
            Self::NativeDiffersFromNode => "NATIVE-vs-NODE",
            Self::EnginesDivergeBothWrong => "ENGINES-DIVERGE",
            Self::BothEnginesDifferFromNode => "BOTH-ENGINES-vs-NODE",
        }
    }

    /// Whether this is a wrong answer in kiln.
    pub const fn is_defect(self) -> bool {
        matches!(
            self,
            Self::InterpreterDiffersFromNode
                | Self::NativeDiffersFromNode
                | Self::EnginesDivergeBothWrong
                | Self::BothEnginesDifferFromNode
        )
    }

    /// Whether this is a wrong answer in `js.rs`. Reported separately because the fix is in a
    /// different file and the two would otherwise be counted together.
    pub const fn is_generator_bug(self) -> bool {
        matches!(self, Self::GeneratorBug)
    }
}

/// One program's three answers, kept whole so a report can be acted on without re-running.
#[derive(Debug, Clone)]
pub struct Verdict {
    pub class: Class,
    /// Why, normalised enough to group by. The conformance README's argument, applied here: the
    /// ranked reason distribution is the more useful half of any such run.
    pub reason: String,
    pub source: String,
    pub node: String,
    /// The first line of what node said about stopping, when it stopped. Empty otherwise.
    pub node_diagnostic: String,
    /// Whether node ran the program to the end.
    pub node_completed: bool,
    pub interpreter: String,
    /// `None` when no native binary was built for this case -- the screen mode, or an emitter
    /// refusal. Reported rather than assumed, so `BOTH-ENGINES-vs-NODE` can never be claimed on a
    /// native answer nobody obtained.
    pub native: Option<String>,
}

impl Verdict {
    /// A short, stable key for deduplicating a soak's findings.
    ///
    /// A soak of ten thousand programs finds one root cause ten thousand times, and ten thousand
    /// copies of one bug is a report nobody reads. The key is the first line on which node and
    /// kiln disagree, from each side, with digits flattened -- so `Math.abs({valueOf:()=>4})` and
    /// `Math.abs({valueOf:()=>9})` land on one entry, and a genuinely different shape does not.
    pub fn signature(&self) -> String {
        // When node stopped and kiln did not, the *output* the two disagree on is whatever the
        // generated program happened to print next, and grouping on that splits one root cause
        // across a hundred entries -- measured: 517 cases of one bug arrived as 155 findings.
        // What they share is node's reason for stopping, so that is the key.
        if !self.node_completed {
            return format!(
                "{} where node stopped: {}",
                self.class.label(),
                flatten(&self.node_diagnostic)
            );
        }
        let kiln = self.native.as_ref().unwrap_or(&self.interpreter);
        let (node_line, kiln_line) = first_difference(&self.node, kiln);
        format!(
            "{} node={} kiln={}",
            self.class.label(),
            flatten(&node_line),
            flatten(&kiln_line)
        )
    }
}

/// The first line on which two outputs differ, from each side.
fn first_difference(left: &str, right: &str) -> (String, String) {
    let mut lefts = left.lines();
    let mut rights = right.lines();
    loop {
        match (lefts.next(), rights.next()) {
            // Every line matched and the two are still not equal, so what differs is not a line:
            // a trailing newline, or a carriage return, which `str::lines` folds away. Saying so
            // beats a signature reading `node=<end> kiln=<end>`, which names nothing.
            (None, None) => {
                return (
                    format!("<same lines, {} bytes>", left.len()),
                    format!("<same lines, {} bytes>", right.len()),
                );
            }
            (left, right) if left == right => {}
            (left, right) => {
                return (
                    left.unwrap_or("<end>").to_owned(),
                    right.unwrap_or("<end>").to_owned(),
                );
            }
        }
    }
}

/// Digits and quoted contents replaced, so one root cause is one entry.
fn flatten(line: &str) -> String {
    let mut out = String::with_capacity(line.len().min(80));
    let mut last_was_digit = false;
    for character in line.chars().take(80) {
        let is_digit = character.is_ascii_digit();
        if is_digit {
            if !last_was_digit {
                out.push('N');
            }
        } else {
            out.push(character);
        }
        last_was_digit = is_digit;
    }
    out
}

/// How one side compares against the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agreement {
    /// Same bytes, both ran to the end.
    Same,
    /// The side stopped early having printed a prefix of the reference. A backlog entry.
    Prefix,
    /// A wrong answer.
    Differs,
}

fn agreement(
    reference: &str,
    reference_completed: bool,
    side: &str,
    side_completed: bool,
) -> Agreement {
    match (reference_completed, side_completed) {
        (true, true) => {
            if reference == side {
                Agreement::Same
            } else {
                Agreement::Differs
            }
        }
        // node finished and kiln gave up part way. A prefix is a refusal; anything else is a wrong
        // answer wearing a refusal's clothes.
        (true, false) => {
            if reference.starts_with(side) {
                Agreement::Prefix
            } else {
                Agreement::Differs
            }
        }
        // node threw and kiln ran to the end: kiln produced a value node says the program does not
        // have. Always a defect, the same way `engines.rs` treats a native binary finishing where
        // the interpreter refused.
        (false, true) => Agreement::Differs,
        // Both stopped, possibly at different points, so neither side owns the longer output.
        // Requiring one to prefix the other is the strongest claim the pair supports.
        (false, false) => {
            if reference.starts_with(side) || side.starts_with(reference) {
                Agreement::Prefix
            } else {
                Agreement::Differs
            }
        }
    }
}

/// What the native side had to say, if it was asked.
#[derive(Debug, Clone)]
pub enum NativeAnswer {
    /// Not built: the screen mode, or a case that did not earn a C compilation.
    NotAsked,
    /// The emitter or the runtime refused. A backlog entry, and no evidence about correctness.
    Refused(String),
    /// It ran. `completed` is false when it exited non-zero, which for the emitted launcher means
    /// either an uncaught exception or `KILN_C99_CONTROL_ERROR`.
    Ran { stdout: String, completed: bool },
}

/// Put one program's three answers together.
///
/// `interpreted` is the interpreter's own result rather than a string, because whether it *ended*
/// is half of every judgement above and a `String` cannot carry that.
pub fn triage(
    source: &str,
    node: &NodeRun,
    interpreted: &Interpreted,
    native: &NativeAnswer,
) -> Verdict {
    let verdict =
        |class: Class, reason: String, interpreter: String, native: Option<String>| Verdict {
            class,
            reason,
            source: source.to_owned(),
            node: node.stdout.clone(),
            node_diagnostic: node.diagnostic.clone(),
            node_completed: node.completed(),
            interpreter,
            native,
        };

    match node.status {
        NodeStatus::Invalid => {
            return verdict(
                Class::GeneratorBug,
                format!("node refused the generated source: {}", node.diagnostic),
                String::new(),
                None,
            );
        }
        NodeStatus::Unusable => {
            return verdict(
                Class::NotComparable,
                format!("node could not answer: {}", node.diagnostic),
                String::new(),
                None,
            );
        }
        NodeStatus::Ok | NodeStatus::Threw => {}
    }

    let (kiln_stdout, kiln_completed, kiln_reason) = match interpreted {
        Interpreted::Completed(stdout) => (stdout.clone(), true, String::new()),
        Interpreted::Stopped { reason, stdout } => (stdout.clone(), false, reason.clone()),
        Interpreted::OverBudget => {
            return verdict(
                Class::NotComparable,
                "the interpreter's screening budget stopped the program".to_owned(),
                String::new(),
                None,
            );
        }
    };

    let interpreter = agreement(&node.stdout, node.completed(), &kiln_stdout, kiln_completed);
    let native_refused = matches!(native, NativeAnswer::Refused(_));
    let (native_agreement, native_stdout) = match native {
        NativeAnswer::NotAsked => (None, None),
        // A refusal is not an answer, so it is not evidence either way -- and in particular it can
        // never be counted as the native backend "agreeing" with the interpreter.
        NativeAnswer::Refused(_) => (None, None),
        NativeAnswer::Ran { stdout, completed } => (
            Some(agreement(
                &node.stdout,
                node.completed(),
                stdout,
                *completed,
            )),
            Some(stdout.clone()),
        ),
    };

    let engines_agree = native_stdout
        .as_ref()
        .is_none_or(|stdout| *stdout == kiln_stdout);

    let class = match (interpreter, native_agreement) {
        (Agreement::Differs, Some(Agreement::Differs)) => {
            if engines_agree {
                Class::BothEnginesDifferFromNode
            } else {
                Class::EnginesDivergeBothWrong
            }
        }
        (Agreement::Differs, _) => Class::InterpreterDiffersFromNode,
        (_, Some(Agreement::Differs)) => Class::NativeDiffersFromNode,
        (Agreement::Prefix, _) | (_, Some(Agreement::Prefix)) => Class::Refused,
        // The interpreter matched node and the emitter would not compile the bytecode. Nothing is
        // wrong, and nothing was compared either: counting it as agreement would inflate the one
        // number a soak is read for.
        (Agreement::Same, _) if native_refused => Class::Refused,
        (Agreement::Same, _) => Class::Agrees,
    };

    let reason = match class {
        Class::Refused => {
            if kiln_completed {
                match native {
                    NativeAnswer::Refused(why) => format!("native refused: {why}"),
                    NativeAnswer::Ran { .. } => "native stopped at run time".to_owned(),
                    NativeAnswer::NotAsked => "a kiln engine stopped early".to_owned(),
                }
            } else {
                format!("interpreter stopped: {kiln_reason}")
            }
        }
        // A defect keeps a reason as well, and it is node's: `node stopped with a TypeError and
        // kiln did not` is one backlog line, where the outputs the two went on to disagree about
        // are one per program.
        _ if class.is_defect() && !node.completed() => {
            format!("node stopped and kiln did not: {}", node.diagnostic)
        }
        _ => String::new(),
    };

    verdict(class, reason, kiln_stdout, native_stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(stdout: &str, status: NodeStatus) -> NodeRun {
        NodeRun {
            stdout: stdout.to_owned(),
            diagnostic: "TypeError: an example".to_owned(),
            status,
        }
    }

    #[test]
    fn two_engines_agreeing_with_each_other_and_not_with_node_is_the_headline_class() {
        let verdict = triage(
            "src",
            &node("99\n", NodeStatus::Ok),
            &Interpreted::Completed("NaN\n".to_owned()),
            &NativeAnswer::Ran {
                stdout: "NaN\n".to_owned(),
                completed: true,
            },
        );
        assert_eq!(verdict.class, Class::BothEnginesDifferFromNode);
        assert!(verdict.class.is_defect());
    }

    #[test]
    fn an_engine_that_stopped_early_is_a_backlog_entry_and_not_a_wrong_answer() {
        let verdict = triage(
            "src",
            &node("1\n2\n", NodeStatus::Ok),
            &Interpreted::Stopped {
                reason: "unsupported".to_owned(),
                stdout: "1\n".to_owned(),
            },
            &NativeAnswer::NotAsked,
        );
        assert_eq!(verdict.class, Class::Refused);
        assert!(!verdict.class.is_defect());
    }

    #[test]
    fn an_engine_that_stopped_early_having_printed_something_else_is_still_a_wrong_answer() {
        let verdict = triage(
            "src",
            &node("1\n2\n", NodeStatus::Ok),
            &Interpreted::Stopped {
                reason: "unsupported".to_owned(),
                stdout: "9\n".to_owned(),
            },
            &NativeAnswer::NotAsked,
        );
        assert_eq!(verdict.class, Class::InterpreterDiffersFromNode);
    }

    #[test]
    fn finishing_where_node_threw_is_a_wrong_answer() {
        // `({valueOf(){return{}}, toString(){return{}}}) + 1` is this shape: node raises a
        // TypeError and kiln prints `[object Object]1`.
        let verdict = triage(
            "src",
            &node("", NodeStatus::Threw),
            &Interpreted::Completed("[object Object]1\n".to_owned()),
            &NativeAnswer::NotAsked,
        );
        assert_eq!(verdict.class, Class::InterpreterDiffersFromNode);
    }

    #[test]
    fn node_refusing_the_source_is_a_generator_bug_and_says_nothing_about_kiln() {
        let verdict = triage(
            "let =;",
            &node("", NodeStatus::Invalid),
            &Interpreted::Completed(String::new()),
            &NativeAnswer::NotAsked,
        );
        assert_eq!(verdict.class, Class::GeneratorBug);
        assert!(!verdict.class.is_defect());
        assert!(verdict.class.is_generator_bug());
    }

    #[test]
    fn a_native_refusal_is_never_counted_as_the_two_engines_agreeing() {
        let verdict = triage(
            "src",
            &node("99\n", NodeStatus::Ok),
            &Interpreted::Completed("NaN\n".to_owned()),
            &NativeAnswer::Refused("unsupported instruction".to_owned()),
        );
        assert_eq!(verdict.class, Class::InterpreterDiffersFromNode);
        assert!(verdict.native.is_none(), "no native answer was obtained");
    }

    #[test]
    fn one_root_cause_reached_by_different_values_has_one_signature() {
        let make = |node_out: &str, kiln_out: &str| {
            triage(
                "src",
                &node(node_out, NodeStatus::Ok),
                &Interpreted::Completed(kiln_out.to_owned()),
                &NativeAnswer::NotAsked,
            )
            .signature()
        };
        assert_eq!(make("4\n", "NaN\n"), make("917\n", "NaN\n"));
        assert_ne!(make("4\n", "NaN\n"), make("4\n", "[object Object]\n"));

        // And the case that made this necessary: node stopping and kiln carrying on. What those
        // cases share is node's reason, not the bytes kiln went on to print, so they group by it.
        let after_a_throw = |kiln_out: &str| {
            triage(
                "src",
                &node("", NodeStatus::Threw),
                &Interpreted::Completed(kiln_out.to_owned()),
                &NativeAnswer::NotAsked,
            )
            .signature()
        };
        assert_eq!(after_a_throw("undefined\n"), after_a_throw("0\ntfalse\n"));
    }

    #[test]
    fn agreement_is_only_claimed_over_the_prefix_that_was_actually_compared() {
        let verdict = triage(
            "src",
            &node("1\n", NodeStatus::Threw),
            &Interpreted::Stopped {
                reason: "threw".to_owned(),
                stdout: "1\n".to_owned(),
            },
            &NativeAnswer::NotAsked,
        );
        assert_eq!(verdict.class, Class::Refused);
    }
}

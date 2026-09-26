//! Grouping the reasons a corpus did not pass.
//!
//! A conformance run over a corpus Kiln cannot execute yet produces one number worth almost
//! nothing — the pass count — and one thing worth a great deal: the distribution of reasons, which
//! is the interpreter's backlog sorted by how many tests each entry unblocks.  This turns a batch
//! of results into that distribution.
//!
//! Grouping is derived from each result's decisive diagnostic rather than from a field the runners
//! set, so both metadata sources group the same way and neither can forget to classify itself.

use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};

use crate::case::{CaseResult, DiagnosticPhase, Outcome};

/// One line of the distribution: an outcome, why, and which tests it accounts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonGroup {
    pub outcome: Outcome,
    /// `phase: message`, or just the outcome's tag when a result carried no diagnostic.
    pub reason: String,
    /// Test ids, in discovery order.
    pub tests: Vec<String>,
}

impl ReasonGroup {
    pub fn count(&self) -> usize {
        self.tests.len()
    }
}

/// Every non-passing result, grouped by reason, largest group first.
///
/// Passes are excluded: a pass has no reason, and a distribution that lists them buries the
/// backlog under the part of the corpus that already works.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReasonReport {
    groups: Vec<ReasonGroup>,
}

impl ReasonReport {
    /// Group results supplied as `(test id, result)` pairs.
    pub fn from_results<'a, I>(results: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a CaseResult)>,
    {
        let mut grouped: BTreeMap<(Outcome, String), Vec<String>> = BTreeMap::new();
        for (id, result) in results {
            if result.outcome == Outcome::Pass {
                continue;
            }
            grouped
                .entry((result.outcome, reason_of(result)))
                .or_default()
                .push(id.to_owned());
        }

        let mut groups: Vec<_> = grouped
            .into_iter()
            .map(|((outcome, reason), tests)| ReasonGroup {
                outcome,
                reason,
                tests,
            })
            .collect();
        // Largest first, because that is the order the backlog should be worked in.  The
        // secondary key keeps equal-sized groups from reordering between runs.
        groups.sort_by(|left, right| {
            right
                .count()
                .cmp(&left.count())
                .then_with(|| (left.outcome, &left.reason).cmp(&(right.outcome, &right.reason)))
        });
        Self { groups }
    }

    pub fn groups(&self) -> &[ReasonGroup] {
        &self.groups
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// One line per group: `count  outcome  reason`.  Fixed leading columns so that sorting, counting
/// and diffing the rendered form all work without a parser.
impl Display for ReasonReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for group in &self.groups {
            writeln!(
                f,
                "{:>5}  {:<11}  {}",
                group.count(),
                group.outcome.tag(),
                group.reason
            )?;
        }
        Ok(())
    }
}

/// The reason a single result is grouped under.
///
/// The compiler's own words are kept verbatim: "unsupported expression: computed property access"
/// is the backlog entry, and any normalisation would blur two refusals into one row.
fn reason_of(result: &CaseResult) -> String {
    match result.primary_diagnostic() {
        // A stdout diff names two whole streams, which is per-test detail rather than a group.
        // The group is the fact that the program ran and printed something else.
        Some(diagnostic) if diagnostic.phase == DiagnosticPhase::Expectation => {
            format!("{}: stdout mismatch", diagnostic.phase.tag())
        }
        Some(diagnostic) => format!("{}: {}", diagnostic.phase.tag(), diagnostic.message),
        None => format!("{} without a recorded reason", result.outcome.tag()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{Diagnostic, Timing};
    use std::time::Duration;

    fn result(outcome: Outcome, diagnostic: Option<(DiagnosticPhase, &str)>) -> CaseResult {
        CaseResult {
            outcome,
            diagnostics: diagnostic
                .map(|(phase, message)| vec![Diagnostic::new(phase, message, None)])
                .unwrap_or_default(),
            timing: Timing {
                elapsed: Duration::ZERO,
            },
            stdout: String::new(),
        }
    }

    #[test]
    fn identical_refusals_collapse_into_one_ranked_group() {
        let refusal = result(
            Outcome::Fail,
            Some((DiagnosticPhase::Compile, "unsupported expression: `typeof`")),
        );
        let other = result(
            Outcome::Fail,
            Some((DiagnosticPhase::Runtime, "step limit exceeded")),
        );
        let report = ReasonReport::from_results([
            ("a", &refusal),
            ("b", &other),
            ("c", &refusal),
            ("d", &refusal),
        ]);

        let groups = report.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].count(), 3, "the largest group leads");
        assert_eq!(
            groups[0].reason,
            "compile: unsupported expression: `typeof`"
        );
        assert_eq!(groups[0].tests, ["a", "c", "d"]);
        assert_eq!(groups[1].reason, "runtime: step limit exceeded");
    }

    #[test]
    fn passes_are_not_a_reason_and_skips_are_kept_apart_from_failures() {
        let passed = result(Outcome::Pass, None);
        let skipped = result(
            Outcome::Skipped,
            Some((DiagnosticPhase::Harness, "needs a bundle")),
        );
        let failed = result(
            Outcome::Fail,
            Some((DiagnosticPhase::Harness, "needs a bundle")),
        );
        let report = ReasonReport::from_results([
            ("passing", &passed),
            ("skipped", &skipped),
            ("failed", &failed),
        ]);

        assert_eq!(report.groups().len(), 2, "a pass contributes no group");
        let rendered = report.to_string();
        assert!(rendered.contains("fail"), "{rendered}");
        assert!(rendered.contains("skipped"), "{rendered}");
        assert_eq!(rendered.lines().count(), 2);
    }

    #[test]
    fn a_stdout_diff_groups_by_kind_not_by_the_two_streams() {
        let first = result(
            Outcome::Fail,
            Some((
                DiagnosticPhase::Expectation,
                "stdout mismatch: expected \"a\", got \"b\"",
            )),
        );
        let second = result(
            Outcome::Fail,
            Some((
                DiagnosticPhase::Expectation,
                "stdout mismatch: expected \"c\", got \"d\"",
            )),
        );
        let report = ReasonReport::from_results([("first", &first), ("second", &second)]);
        assert_eq!(report.groups().len(), 1);
        assert_eq!(report.groups()[0].reason, "expectation: stdout mismatch");
    }

    #[test]
    fn an_empty_run_renders_to_nothing() {
        assert!(ReasonReport::default().is_empty());
        assert_eq!(ReasonReport::from_results([]).to_string(), "");
    }
}

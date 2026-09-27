# ADR 0020: Conformance is measured by specification coverage, not by pass rate

## Status

Accepted. This extends ADR 0008 rather than replacing it: the four explicit outcomes, the external
Test262 checkout, and the failures-first selector all stand. What it adds is the number the project
steers by, and a stronger differential oracle to feed it.

It lands in `conformance/`, `crates/tools/kiln-conformance`, `crates/tools/kiln-test-selector` and
`docs/language-coverage.md`, which are this session's files under the split recorded in `NOTES.md`.

## Context

The project's largest identified risk is not difficulty, it is surface area: ten thousand small
semantic obligations, each with a Test262 case attached, and whether the work of covering them
compounds or grinds. Every architectural decision made so far — the query database, the resident
daemon, the failures-first selector, the differential harness — exists to make that work compound.
None of them measures whether it is working.

Pass rate cannot be that measure. It saturates: an implementation covering the easy nine tenths of a
feature reports the same shape of number as one covering all of it, and the tests a subset
implementation cannot run are excluded from the denominator, so the metric improves when coverage
narrows. `docs/language-coverage.md` and `status.sh` already refuse to report a pass rate for
exactly this reason, which is correct and leaves the project without a health number.

There is now a published alternative that does not have this property. Feature-sensitive
specification coverage measures which *steps of the mechanized ECMAScript specification* a test
suite exercises, and distinguishes reaching a step through one feature from reaching it through
another — because implementations have specialized paths per feature, and an abstract operation
exercised only via `Array.prototype.map` is not thereby tested via `String.prototype.at`. The
2026 refinement adds selectivity: choosing the feature contexts relevant to the implementation
under test, rather than the full product, cuts synthesized tests by roughly seventy percent at
comparable bug-finding power. The approach is in ECMA-262 and Test262 CI, and the WebAssembly
standard independently arrived at the same practice.

Separately, kiln's interpreter-versus-native equivalence check compares stdout. That is a weak
oracle. Recent differential work on JavaScript engines measured the state entropy of successive
oracle designs and found stdout-level comparison to be examining a small handful of values
repeatedly; comparing canonical execution state instead raised entropy substantially and found bugs
the weaker oracles missed. The dominant cost of that technique elsewhere is suppressing false
positives from ambient nondeterminism — clocks, randomness, stack depth, finalization order — which
ADR 0012 and ADR 0015 have already eliminated here by construction.

## Decision

**Specification-step coverage, feature-sensitive, is the project's primary conformance measure.**
It answers "which specification steps, reached through which features, does our suite exercise", it
is monotone, and it names the untested surface instead of hiding it. Pass rate remains reported —
ADR 0008's four outcomes are unchanged — but it is an outcome, not a target.

**The metric is extracted; the toolchain is not adopted.** The reference implementation is a large
Scala system, and taking a dependency on it would violate ADR 0004's workspace discipline and
ADR 0008's stance that conformance inputs are external and explicit. What kiln needs is the
mechanized specification as data and a coverage counter over it, not the surrounding infrastructure.
Until that extraction exists, `docs/language-coverage.md` remains the honest substitute: a
hand-maintained, measured backlog.

**The selector prioritizes by uncovered specification steps.** `kiln-test-selector` today orders
previously failing tests before smoke and affected tests. Uncovered specification steps become a
further ordering input, so the everyday command spends its budget where the suite is blind rather
than where it has already looked. This is the mechanism by which the metric changes behaviour rather
than merely reporting.

**The interpreter/native differential oracle compares canonical state, not stdout.** A tier-invariant
state dump keyed on bytecode offset — locals, live operand stack, and a bounded object graph —
emitted from the interpreter and from native code at function boundaries and at ADR 0016's guard
sites, replaces stdout equivalence. Dumps are grouped by invocation and compared as sets rather than
matched frame to frame, which is what makes the technique tractable. Serialization excludes
addresses and tier-specific data.

**Coverage of specialization claims is counted as observed `(site, type)` pairs**, per ADR 0016's
requirement that guard sites be countable. Both components are compiler-assigned, so the space is
enumerable and needs no approximation. This measures something spec coverage cannot: which
specialization assumptions have actually been exercised, and with what.

**LLM-generated tests are not adopted as an oracle.** The strongest published system in this area
reports recall between 0.73 and 0.84 with an eighteen percent false-positive rate. A quarter of
misses is not an oracle for a project that ranks correctness first; at most such a system ranks the
output of a sound oracle. The bar for revisiting is explicit: beat a plain seed-corpus mutator over
Test262 seeds on specification-step coverage per CPU-hour. A deliberately unsophisticated mutator
has been shown to beat published LLM test generators by wide margins on exactly that comparison, so
the bar is real rather than rhetorical.

## Consequences

- The project acquires a health number that can be watched over months and that distinguishes
  "implemented the easy part of this feature" from "implemented this feature". That number, not the
  benchmark figures, is the leading indicator of whether the architecture's bet on compounding is
  paying off.
- Extracting the mechanized specification as data is real work, and it is not obviously small. Until
  it exists this ADR describes a target rather than a practice, and `status.sh` should keep refusing
  to report a number it cannot honestly produce.
- The state-dump oracle costs more per test than comparing stdout and finds far more. Kiln pays less
  for it than anyone else because the deterministic realm removes the false-positive filtering that
  dominates the technique elsewhere — a return on ADR 0012 and ADR 0015 that neither anticipated.
- The oracle and the metric both want the same thing from the emitter that ADR 0016 wants: stable,
  addressable guard sites. Three independent uses now depend on that identity, which is an argument
  for establishing it before the first guard rather than after the ten-thousandth.
- Failures become semantic divergences located at a bytecode offset rather than mismatched output.
  The natural deduplication key is the first divergent state in a trace, which the dumps supply
  directly; the crash-triage literature is not applicable and should not be mined.
- The failures-first selector appears to be ahead of the published state of the art — no work was
  found on prioritizing or minimizing Test262 for a partial implementation. There is nothing to
  adopt there, only a better signal to prioritize by.

## Sources

Checked 29 August 2026.

- Feature-sensitive specification coverage: Park, Ryu et al., PLDI 2023; extended as "Selective
  Feature-Sensitive Coverage for Conformance Testing of Programming Languages", TOSEM, April 2026,
  <https://dl.acm.org/doi/10.1145/3808231>. Toolchain: <https://github.com/es-meta/esmeta>.
- Independent replication in another standard: SpecTec, PLDI 2024; WEST, ASE 2025.
- State-dump differential oracle: Wachter, Gremminger, Wressnegger, Payer & Toffalini, "DUMPLING",
  NDSS 2025.
- `(site, type)` coverage: Wienand, Bernhard & Toffalini, "TYPEFUZZ", NDSS FUZZING 2026 — a
  registered report with preliminary results; the metric is borrowed, not its effectiveness claim.
- The baseline that LLM test generation must beat: Xu, Sun et al., "Kitten", ISSTA 2025.
- Recall and false-positive figures for the strongest LLM oracle found: SmartOracle,
  <https://arxiv.org/html/2601.15074> (2026).

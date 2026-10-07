# 03. `kiln build --explain-parallel`

Status: resolved

For every call site of a parallel built-in, report whether it runs in parallel and, if not, the
first thing that prevented it, with its source location:

```
src/report.ts:42:18  parallelMap  sequential
  captures `total` (src/report.ts:40:7), which is reassigned at src/report.ts:47:5
src/report.ts:58:10  parallelMap  parallel (tier 2: reads properties)
```

The reasons come from the same rules that decide (`parallel.rs` and its successors), so the
explanation can never disagree with the decision. Each rule gets a stable reason code.

## Acceptance

- Every rule in `parallel_safe` and in later tiers has a reason code and a test that produces it.
- The report is available from `kiln check` as well as `kiln build`.

## Comments

2026-09-26, branch `work/parallel-tier1`. Status left open: `kiln check` does not have it.

**Reason codes.** `parallel::Reason`, 23 stable kebab-case codes with `code()`, `from_code()`
and `describe()`: the source rules (`not-plain-function`, `arity`, `rest-parameter`,
`exception-handler`, `throws`, `non-numeric-constant`, `unsupported-operation`,
`captures-reassigned`, `captures-unresolved`, `writes-capture`, `reads-global`, `reads-this`,
`reads-property`, `writes-property`, `allocates`, `calls`, `returns-undefined`), the plan's
(`returns-non-number`, `not-typed`, `clone-rejected`) and the call site's (`callback-unknown`,
`extra-arguments`, `generic-only`). `parallel_safe` is now `parallel_verdict(..) ->
Result<ParallelShape, Refusal>` (the refusal carries the reason and what it names: a capture, a
member, a callee, an operation); `parallel_safe` remains as the `bool` wrapper. The verdict per
proto lives in every `CallPlan` (`parallel`, shared through an `Arc`), and `set_parallel` turns a
rejected clone into `clone-rejected`.

**Same code that decides.** One function in `call_method.rs`, `parallel_site`, decides each
`parallelMap` site: the pooled lowering is emitted from its answer, and the same answer is
written into the generated C as a comment beside it (`/* kiln-parallel-site line=6 column=23
verdict=parallel captures=1 */`, `parallel::SiteRecord`). `kiln-native` reads the records back
from the C it compiled -- on a generated-C cache hit too -- into
`NativeBuildReport::parallel_sites`, and the daemon renders them. So the report is the decision
read off the artifact; it cannot disagree with it.

**CLI.** `kiln build --explain-parallel` prints the build summary and then one entry per site;
`--explain` now includes the same section (`parallel:`) ahead of the facts:

```
conformance/native/parallel-capture-const.js:24:19  parallelMap  parallel (tier 1: reads captured values)
conformance/native/parallel-capture-const.js:32:28  parallelMap  sequential
  captures-reassigned: captures `bump` (conformance/native/parallel-capture-const.js:32:49), which is assigned 2 times
conformance/native/parallel-refused-release.js:23:22  parallelMap  sequential
  reads-property: reads property `scale`
```

**Positions.** Bytecode carries spans for functions only, not instructions, and emission packs
the source for `toString`; the original source and spans are kept for the length of a unit's
emission (`parallel::with_original_source`). A site is placed at the `parallelMap` name just
before its callback (else the callback, else the enclosing function), and a capture refusal at
the capture's first use in the callback. The *write* that disqualifies a capture is reported as a
count, not a position -- there is nothing to locate it by yet.

**Tests.** One per code: the seventeen source reasons in `parallel.rs`
(`reason_*`), the six plan and site reasons in `parallel_tests.rs` (from emitted C), round trips
of the codes and of the C marker, and `--explain-parallel` argument parsing in `kiln-cli`.

**Not done: `kiln check`.** `check` runs the query pipeline and never emits C; the verdict needs
the whole emission (graphs, plan, the clone check), and `check` would have to run
`kiln-native`'s graph emission path to answer. That is the remaining acceptance item.

**Worth knowing.** The report makes two things visible: `kiln build`'s default profile is
`native-dev` (every site `generic-only`), and a release module past 16 functions or 512
instructions is also generic-only -- so most real modules get no parallel lowering today,
whatever their callbacks. That limit, not the tier rules, is the first obstacle for PRD's success
measure.

2026-09-27, branch `work/parallel-cost-model`. `kiln check --explain-parallel` exists; status
resolved.

**Design: `check` runs the build and stops after emission.** The verdict is made during emission
(`parallel_site` in `call_method.rs`, over the reduced graph, its live slots and the profile's
emission policy), and read back from the emitted C. Re-running only `parallel_verdict` /
`plan_parallel_sites` outside that pipeline would feed the rules different inputs (no demand
reduction, no live slots, no profile policy) and could disagree with the build. So
`NativeBuilder::build` and `build_graph` became thin wrappers over `build_until` /
`build_graph_until`, which take a `BuildUntil::{Executable, Emitted}`; with `Emitted` they return
right after the site records are read (before the launcher, the workspace, the C compiler or the
linker). `NativeBuilder::parallel_sites` / `parallel_sites_graph` are that stop. It is literally
the build's own code up to that point, including the generated-C cache, so a `check` followed by
a `build` (or the other way round) emits once.

**Wire and CLI.** New `Request::ExplainParallel { path, project_root, node, profile, source_view }`
answered by `Response::ParallelExplained { text }` (the `parallel:` section,
`format_explain_section`); `PROTOCOL_VERSION` 33 -> 34. The daemon serves it from the same
snapshot preparation as `NativeBuild` (`prepare_native_snapshot`, graph or single program), off
the engine lock. `kiln check <file> --explain-parallel [--profile <p>]` runs the check as before,
then asks at the same source view and prints the sites after it exactly as `kiln build
--explain-parallel` does. The profile defaults to `kiln build`'s, so the two explain the same
build unless told otherwise; `--profile` is accepted by `check` only with `--explain-parallel`.
A file with diagnostics gets no report (there is no build to explain). No C compiler runs, but
the toolchain is still discovered, since its identity is part of the generated-C cache key.

```
$ kiln check sites.mjs --explain-parallel --profile native-release
sites.mjs  r3  1.74ms
  ...
sites.mjs:5:16  parallelMap  parallel
sites.mjs:6:16  parallelMap  sequential
  captures-reassigned: captures `bump` (sites.mjs:6:37), which is assigned 2 times
sites.mjs:7:16  parallelMap  sequential
  reads-property: reads property `k`
```

Identical to `kiln build --explain-parallel` for the same file and profile (and for the default
profile, where every site reads `dev-profile`). The check took 50 ms wall with a cold
generated-C cache, one-shot: emission is all it adds.

**Tests.** `parallel_sites_without_compiling_are_the_builds_own` (kiln-native): a graph with a
parallel site and a `captures-reassigned` one; `parallel_sites_graph` produces no executable and
equals `build_graph(..).parallel_sites`. Argument parsing (`check` with and without the flag,
profile default and override, `--profile` refused without it), and the wire shape round trip in
kiln-proto.


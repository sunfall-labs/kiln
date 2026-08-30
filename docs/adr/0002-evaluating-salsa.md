# 0002. Evaluating salsa as the backing implementation of `QueryEngine`

## Status

Accepted. **Verdict: adopt later, behind the trait. Not now.**

Evaluated against salsa 0.28.2, published 3 August 2026, which is the current release. Anything
below that describes salsa's behaviour should be re-checked before acting on it, because salsa
is moving fast and broke its public API twice in the eight weeks before this was written.

Re-checked 30 August 2026, verdict unchanged. Re-takes are logged under "Re-evaluation log" below
rather than by editing the evaluation, so the history of the answer stays visible.

## Context

Design.md §2 names salsa directly: "Salsa is worth evaluating for this because it memoizes query
results and recomputes only affected values, although its repository still describes it as work
in progress. We should hide it behind our own query interface so the compiler is not permanently
tied to it."

ADR 0001 built that interface. This ADR is the evaluation the interface was created to make
possible, and it carries the list of requirements ADR 0003 refers to when it calls early cutoff
"the hardest requirement in ADR 0002's list".

### What we require of a backend

These are the things `QueryEngine` and the design document actually demand. They are the
criteria; everything after this section is salsa measured against them.

1. **Early cutoff.** Recomputing a query to an equal value must not invalidate its dependents.
   ADR 0003 makes design.md §3's interface-hash rule a consequence of this rather than a rule
   anyone follows, so a backend without it breaks §3 silently — every signature still compiles,
   every correctness test still passes, and the compiler is just slower forever.
2. **Observable recompute accounting.** `Stats` is a first-class output, not logging. The tests
   and the timing harness assert on `parse_computed`, `project_computed`, `validated` and
   `early_cutoff`. A backend we cannot count is a backend whose incrementality we cannot defend.
3. **Values that survive a write.** `QueryEngine` hands back owned `Parsed`, `Interface` and
   `Project` values, and callers hold them across `set_source`. The daemon serialises them onto
   the wire as `kiln-proto` types.
4. **Cheap edit-time rebuilds of whatever declares queries.** Design.md §12: "Rust rebuild time
   must be monitored continuously", "minimize procedural macros in frequently edited crates."
   Iteration speed ranks second, above architectural flexibility and above native runtime
   performance. A backend that taxes every `cargo check` of the compiler's hot crates is being
   paid for out of priority 2 to buy something in priority 4.
5. **Cancellation and concurrent reads.** Design.md §1 is a long-running daemon serving a CLI,
   a watcher and eventually an editor. An edit arriving mid-query must cancel it, not queue
   behind it.
6. **Durability tiers.** Design.md §7: "Built-in and npm dependency artifacts should be highly
   durable because they change far less often than application code." A backend that treats
   `node_modules` and the file under the cursor identically rescans the world on every keystroke.
7. **Replaceability.** ADR 0001's whole argument. Adopting a backend must not be a one-way door.

### Which salsa this is

Be precise here, because most writing about salsa online is about an API that no longer exists.
The old one — `#[salsa::query_group]`, a `#[salsa::database]` struct listing storage, and
`Snapshot<DB>` for parallelism — is gone. The rewrite that replaced it was tracked as "Salsa
2022" and ships under the `Salsa 3.0` milestone, which is 82% complete with three open issues
and no due date. Version numbers never crossed 1.0.

The dates are worth having, because they show how young the thing being evaluated actually is.
0.16.1 (May 2021) was the last release of the old API. The redesign appeared as `0.17.0-pre.1`
in May 2021 and then sat in pre-release for nearly four years; the first stable release of the
new API was **0.18.0 on 20 February 2025**. Since then there have been eleven minor releases in
eighteen months, and 0.28.2 is the current one. Almost everything written about salsa predates
February 2025 and describes an API that no longer compiles.

The current API is:

- `#[salsa::input]` on a struct — the base inputs. Fields get generated getters and, via the
  `Setter` trait, `set_field(&mut db).to(value)` setters. `#[salsa::input(singleton)]` for
  a one-instance input, which is how a workspace file list is modelled.
- `#[salsa::tracked]` on a free function, an inherent or trait `impl`, or a struct. Tracked
  functions take `&dyn Database` (or a `#[salsa::db]` subtrait) as the first argument and are
  keyed by their remaining arguments.
- `#[salsa::interned]` for canonicalised values, `#[salsa::accumulator]` for side-channel
  diagnostics, `#[salsa::db]` on the database struct and on any database subtrait.
- `salsa::Storage<Self>` as a field of the database struct; `Storage: Clone`, and cloning it is
  how a second thread gets a handle.

MSRV is 1.85 and salsa is itself edition 2024, so it is compatible with this workspace's
toolchain.

Three signals about maturity, all from the 0.28.2 artifact itself rather than from commentary:
the README still opens with "Obligatory warning — Very much a WORK IN PROGRESS at this point";
the crate description is still "A generic framework for on-demand, incrementalized computation
(experimental)"; and the default feature set includes `salsa_unstable`, whose manifest comment
reads `# FIXME: remove salsa_unstable before 1.0`. Design.md §2's parenthetical is still
accurate, three years and one rewrite later.

The churn is not historical. In the eight weeks before this was written, 0.28.0 (12 July 2026)
replaced the `Update` trait with `SalsaValue` plus `PartialEq` and changed tracked functions and
field getters to **return references by default**. Both are breaking changes to the shape of
ordinary user code, in a patch-numbered pre-1.0 release, in the most recent minor version.

## Evaluation

### 1. Early cutoff: yes, and it is exactly the mechanism ADR 0003 assumes

Salsa calls it *backdating*: "when we mark a value that was computed in revision R as having last
changed in some earlier revision." The crate documentation states the rule directly:

> After re-execution, Salsa compares the old and new results with `PartialEq`. If they are equal,
> Salsa preserves the memo's previous "changed at" revision. This optimization is called
> backdating; it prevents invalidation from propagating to dependents when the result has not
> changed. The `no_eq` option disables this comparison.

This was not taken on trust. Kiln's exact pipeline — `parse(file)` → `interface(file)` →
`project()`, with the same value types — was ported to salsa 0.28.2 as a scratch crate and
instrumented with per-query execution counters. The results:

- An edit inside a function body, changing `implementation_hash` but not the export list,
  re-runs `parse` and re-runs `interface`, and does **not** re-run `project`. This is
  `Stats::early_cutoff` behaviour, obtained for free.
- An edit that adds an export re-runs all three.
- Writing byte-identical text back over a file still re-runs `parse` — salsa does not
  deduplicate input writes — and still cuts off before `project`. That is precisely the
  behaviour `QueryEngine::set_source` documents: "Bumps the revision even if the text is
  unchanged; the engine is expected to cut that off downstream rather than pretend the write did
  not happen."

Requirement 1 is met, and met in the same shape our own trait describes. This is the strongest
argument in salsa's favour and the reason it is worth keeping on the table at all.

### 2. Recompute accounting: partial, and the gap is `early_cutoff`

Salsa's database can install an event callback receiving `EventKind`. Two variants map onto our
counters: `WillExecute { database_key }` gives `parse_computed` / `interface_computed` /
`project_computed`, and `DidValidateMemoizedValue { database_key }` gives `validated`. Both carry
a `DatabaseKeyIndex`, so an adapter can attribute them per query.

There is no event for backdating. `Stats::early_cutoff` — the counter ADR 0003 exists to make
observable, and the one that catches a backend quietly losing cutoff — has no direct API.

There is a sound indirect hook, and it was verified rather than assumed. Instrumenting
`PartialEq for Interface` by hand and counting calls shows that salsa invokes `eq` exactly once
per re-execution that has a prior memo to compare against, and never otherwise: not on first
execution, not on a validated read, not on a read at an unchanged revision. So `eq` returning
`true` *is* a cutoff, one-for-one, and an instrumented `PartialEq` is a correct implementation of
the counter.

It is also fragile in a specific way worth naming: it requires hand-writing `PartialEq` for every
memoized value type, and the day somebody replaces one with `#[derive(PartialEq)]` — which is
strictly more idiomatic and passes every test — the counter silently reads zero and the timing
harness silently stops guarding anything. If we adopt salsa, that instrumentation needs a test
that fails when the hand-written impl goes away.

### 3. What salsa requires of query results

- **`'static` or `SalsaValue`.** Ordinary owned values are accepted directly. Types carrying the
  database lifetime must implement the `unsafe trait SalsaValue`, derivable, which asserts the
  value stays safe to retain into a later revision. Our `Parsed` / `Interface` / `Project` are
  plain owned data, so this costs nothing today. It starts to matter when HIR and IR values want
  to hold interned handles.
- **`PartialEq`, or no cutoff.** Backdating is the `PartialEq` comparison; `no_eq` opts out of
  the comparison and thereby out of early cutoff for that query. This is the same constraint ADR
  0003 already accepted for `MemoEngine`, phrased identically. No new cost.
- **Return mode.** Since 0.28.0 the default is `ref`: a tracked function returns a reference into
  the memo, tied to the immutable database borrow. That is faster and it is a real ergonomic
  cliff. In the port, `let p = project(&db, ws); a.set_text(&mut db).to(...);` fails to borrow-check,
  because the memoized result is still borrowing the database. `QueryEngine` returns owned values
  that callers hold across `set_source`, so every kiln query would carry `returns(clone)` or
  `returns(copy)`. Requirement 3 is satisfiable but not by default; the adapter pays a clone
  where `MemoEngine` also pays one, so this is a wash on cost and a papercut on ergonomics.
- **Multi-argument queries intern their key.** A tracked function with two or more non-database
  arguments interns the argument tuple on every call to get a query key, so those arguments need
  `Eq` and `Hash` and the call pays an interning lookup. Design.md §2 keys results on target and
  optimization profile alongside the file, so most of our real queries will be multi-argument and
  will take this path.
- **Interned handles expire.** Interned values carry a `'db` lifetime and low-durability ones are
  reclaimed after `revisions` (default 3) unused revisions, reusing the slot with a new
  generation. Any cache of our own that outlives a revision must not hold them.

### 4. Compile-time cost: the number design.md §12 asks for

Salsa is macro-heavy by construction, and §12 asks us to minimise procedural macros in frequently
edited crates. So it was measured rather than argued about.

Method: two single-crate projects, identical query pipelines, one using `#[salsa::tracked]` and
one hand-written with no dependencies; `cargo check` and `cargo build` in the dev profile with
incremental on, after appending one trivial function to the crate's own source; five to seven
edits each, median reported. Apple M5 Pro, 18 cores, rustc 1.97.1, macOS 25.6.

Marginal cost per query, on the crate that declares the queries:

| tracked fns in the crate | `cargo check` with salsa | `cargo check` hand-written |
| ------------------------ | ------------------------ | -------------------------- |
| 10                       | 149 ms                   | 83 ms                      |
| 50                       | 228 ms                   | 89 ms                      |
| 150                      | 421 ms                   | 95 ms                      |

That is **≈1.9 ms of `cargo check` per `#[salsa::tracked]` function, per edit, forever** — the
hand-written version costs ≈0.09 ms per function, i.e. essentially nothing. The cost is linear
and it is paid on every keystroke-adjacent rebuild of that crate.

Codegen is much worse than checking. At 150 queries, an incremental dev-profile `cargo build` of
the same crate takes **945 ms with salsa against 155 ms without**. Salsa's own changelog is
consistent with this: 0.26 through 0.28 contain something like fifteen separate
"reduce monomorphization" commits. They are winning, and the cost is still six-fold.

One-time costs, on top of that:

- Salsa pulls **38 transitive crates**, including rayon, parking\_lot, hashbrown, indexmap,
  crossbeam, tracing, inventory and syn 3.
- A cold dev build of salsa and its dependencies is **≈4.4 s**. Except that nobody actually runs
  salsa unoptimised: **both** rust-analyzer and ruff/ty pin `salsa.opt-level = 3` in their dev
  profiles, because a debug-built salsa is too slow to use. With that pin the cold build is
  **≈8–11 s**, and it is re-paid on every toolchain bump and feature change.

Read against §12, this is the strongest argument *against* adopting now. Note also rust-analyzer's
own manifest comment, which is a direct statement from the largest consumer:
`# Ideally we'd not enable the macros feature but unfortunately the tracked attribute does not
work on impls without it`. The proc macro is not optional in practice.

The right way to hold this cost, if we take it, is containment rather than avoidance: the crate
that declares salsa queries is not the crate anyone edits while iterating on a pass. That is a
non-trivial constraint on the layout in ADR 0004 and it should be decided deliberately, not
discovered.

### 5. Cycles, cancellation and parallelism

**Cycles.** Default behaviour is to panic on re-entering an active query. Fixpoint iteration is
opt-in per query via `cycle_initial` (seed value) and `cycle_fn` (called with the provisional
value and the iteration count, returns the value to use). Convergence is detected when
`cycle_fn` returns the previous iteration's value; there is also a non-iterating
`cycle_result` fallback mode. `MAX_ITERATIONS` is 200, after which salsa panics. This is real,
tested machinery — twenty-one dedicated cycle test files, plus eighteen more under
`tests/parallel` covering cycles and cancellation across threads — and it is more than a
hand-written engine will have for a long time. Kiln does not need it yet; a type checker for
TypeScript's structural, mutually recursive types eventually will.

**Cancellation.** Taking `&mut db` (which is what an input setter does) sets a cancellation flag;
active queries on other handles observe it and unwind with a `Cancelled` payload thrown by
`panic::resume_unwind`, caught by `Cancelled::catch`. Since 0.26 there is also a
`CancellationToken` for cancelling one specific local database's computation. The `Cancelled`
variants — `Local`, `PendingWrite`, `PropagatedPanic` — are exactly the daemon's vocabulary from
design.md §1. The catch is that cancellation is an unwind: any `catch_unwind` we add in the
daemon must re-raise a `Cancelled` payload rather than turn it into an error response, or writes
will hang waiting for handles that never noticed they were cancelled.

**Parallelism.** The model is one `Storage` clone per thread over a shared `Arc<Zalsa>`. A write
takes `&mut`, cancels all other handles and blocks until they drop — which the API documents as a
deadlock hazard if the blocking thread holds a handle itself. This is the rust-analyzer model
and it works, but it is a discipline the daemon has to adopt wholesale rather than adapt.

**Prior art.** rust-analyzer completed its port to the new salsa (PR #18964) and shipped a
regression with it: issue #19404 reported `parallel_prime_caches` going from 26 s to 112 s on a
100-crate monorepo — 4.3× — plus indefinite hangs at full CPU, fixed later in PR #19474. That is
the honest shape of the risk: salsa is correct, and its performance characteristics under
concurrency are subtle enough to surprise the people who maintain it. On the other side of the
ledger, both rust-analyzer and Astral's `ty` are on **0.28.2** — today's release — which means
this dependency is exercised at a scale kiln will not reach for years, and Astral is upstreaming
fixes.

### 6. Durability: salsa has it, `MemoEngine` does not

`Durability` (LOW / MEDIUM / HIGH / NEVER\_CHANGE) lets salsa skip tracing a query's dependencies
back to base inputs when it knows only low-durability inputs moved. Design.md §7 asks for exactly
this and names exactly this use: built-ins and npm dependencies are durable, application source
is not. Salsa also gives interned values `NEVER_CHANGE` by default.

This is the clearest thing salsa buys that we do not have and have not planned. It is not an
optimisation we can bolt onto `MemoEngine` cheaply — it is a second dimension in the validation
algorithm.

Two more in the same category. `lru = N` plus a `heap_size` hook evicts memoized results under
memory pressure while retaining their metadata, which a long-lived daemon holding an entire
`node_modules` will want. And an opt-in `persistence` feature serialises the whole database
through serde (`<dyn Database>::as_serialize` / `::deserialize`, with `persist` on each
participating item), which is a real answer to design.md §7's cross-session artifact cache and to
what a daemon does after a restart.

### 7. Migration cost away from salsa, later

This is the question ADR 0001's trait boundary was drawn to answer, so it deserves a real answer
rather than "the trait protects us."

The trait genuinely does bound the blast radius: a `SalsaEngine` implementing `QueryEngine` is a
separate crate, no consumer names a salsa type, and `MemoEngine`'s existing tests become the
acceptance suite for the swap in either direction. Removing salsa would mean deleting one crate.

What the trait does **not** protect is the query schema. Salsa's ergonomics push design decisions
into the shape of the queries themselves — what is an input struct versus a tracked struct versus
an interned value, which queries are multi-argument (and therefore intern their keys), where
`'db` lifetimes thread through derived values, which queries carry cycle recovery. A year of
building the frontend against salsa produces a schema shaped by salsa's grain, and unwinding
*that* is the expensive part, not deleting the crate. The `'db` lifetime in particular is
infectious: once HIR nodes are tracked structs, every function that touches HIR carries `'db`,
and a hand-written replacement has to either reproduce that lifetime or rewrite the signatures.

So the boundary makes leaving *possible* and does not make it *cheap*. The cost of leaving grows
with how much of the compiler has been written against it — which is an argument for deciding
early, and, given everything above, for deciding early in the direction of *not yet*.

## Decision

**Adopt later, behind the trait. Not now.**

`MemoEngine` stays the only implementation of `QueryEngine` for v0. Salsa is not added to the
workspace.

The reasoning, ranked the way design.md ranks it:

Salsa's headline feature is early cutoff, and early cutoff is the one thing `MemoEngine` already
provides — it is a value comparison and a `changed_at` revision, and it is a few dozen lines, not
a research problem. The features salsa has that we do not are durability, LRU eviction,
cross-thread cycle recovery, and persistence. Every one of those is a scaling feature: valuable
when the graph is large, worth nothing at three queries and one thread. Meanwhile the cost —
1.9 ms of `cargo check` per query per edit, 6× on incremental codegen, 38 crates, ~10 s of cold
build with the opt-level pin that real consumers find necessary — is charged immediately and
charged against priority 2, which outranks everything salsa is offering.

Adopting a pre-1.0 dependency that broke its core value trait and its default return mode six
weeks ago, in order to obtain a property we already have, in exchange for a permanent tax on the
loop the project is organised around, is the wrong trade at this size. It is very plausibly the
right trade at ten times this size, which is why the verdict is "later" rather than "no".

In the meantime:

- The port used for this evaluation is not checked in. It was a scratch crate; §4's numbers and
  §1's cutoff results are reproducible from the method described above, and re-running it is
  cheap. Do that again rather than trusting these numbers if the decision is being reopened.
- `QueryEngine`'s three documented guarantees are already stated in backend-neutral terms. Keep
  them that way. Where `MemoEngine` gains a capability salsa also has — durability tiers in
  particular — model it on salsa's version, so the eventual adapter is a translation and not a
  redesign.
- Do not let `Stats` become `MemoEngine`-shaped. `early_cutoff` is the counter with no native
  salsa equivalent (§2 above), and it is worth keeping the deliberate knowledge that satisfying
  it costs a hand-written `PartialEq` per memoized type.

## Consequences

We keep a hand-written engine we are responsible for. ADR 0001 already accepted that risk and its
mitigation — the tests assert on recompute counts, not just on outputs — and this decision does
not change it, but it does extend how long we carry it.

We will meet salsa's scaling features later than we would have, and at least one of them will
hurt before we build it. Durability is the likeliest: the first time the daemon holds a real
`node_modules` and revalidation walks all of it on every keystroke, §7's durability tiers stop
being a nice-to-have. Building them into `MemoEngine` is a day or two of work; noticing we need
them will take a benchmark, which is what the timing harness is for.

We are choosing to re-do this evaluation rather than to do it once. That is deliberate — salsa is
improving quickly and about half the objections above are cost objections that its maintainers
are actively working on — but it means this ADR has an expiry date, and a stale "no" is worse
than a considered "yes".

## When to revisit

Any one of these should reopen the decision:

1. **Salsa reaches 1.0**, drops the `salsa_unstable` default feature and the README's
   work-in-progress warning. This removes the churn objection, which is the objection that most
   directly contradicts design.md §2's own reservation.
2. **The query count crosses roughly fifty** and the schema stops changing weekly. Below that,
   `MemoEngine` is smaller than the adapter would be. Above it, hand-maintaining validation for
   a schema with cycles and mixed durability stops being obviously cheaper.
3. **`MemoEngine` needs its second scaling feature.** Durability alone is worth writing. If
   durability *and* LRU eviction *and* cross-thread cycle recovery are all on the list, we are
   reimplementing salsa badly and should stop.
4. **The daemon needs genuinely concurrent queries**, not just a background thread — a
   cancellable read served while another read is in flight. Salsa's cancellation and blocking
   model is subtle, well tested, and not something to write twice.
5. **Salsa's compile-time cost drops materially**, meaning under ~0.5 ms per tracked function on
   `cargo check` and under 2× on incremental codegen, measured by re-running §4's method. This is
   the objection with a number attached, so it is the one that can be settled rather than argued.
6. **The persistent artifact cache of §7 becomes load-bearing** and salsa's `persistence` feature
   is stable. Serialising a dependency graph correctly is the kind of work it is much better to
   inherit than to write.

Conversely, one thing should *close* the question in salsa's favour without waiting: if a future
requirement turns out to need a property salsa has and cutoff-plus-memoization cannot express —
demand-driven cycle resolution in the type checker is the candidate — then we adopt, take the
compile-time hit, and contain it by keeping the query-declaring crate off the hot edit path.

## Re-evaluation log

"When to revisit" is only worth writing if somebody actually revisits, and a criterion nobody
re-takes is indistinguishable from no criterion. This section is where each re-take goes, newest
first. Adding an entry should be cheap; if it ever stops being cheap, that is itself information.

### 2026-08-30 — no change to the verdict

Taken one day after the evaluation above, which makes this entry a check on the *procedure* rather
than on salsa: nothing about a dependency moves in a day. It is here so the log exists with an
entry in it and the next person is copying a format rather than inventing one.

- **The current release is still 0.28.2**, published 3 August 2026, unchanged. Worth stating
  because it is the number most likely to be misremembered: 0.26.0 is February 2026, three minor
  versions back, and predates both of the 0.28.0 breaking changes described under "Which salsa
  this is". Anyone re-reading this ADR against an older version number is reading about a
  different API, which is the same trap "Which salsa this is" exists to warn about.
- **Still pre-1.0, and there is still no 1.0 milestone.** The open milestones are `Salsa 3.0`
  (3 open / 14 closed, no due date) and `Far future` (1 open / 2 closed). Neither is a
  stable-release plan, and no published version has crossed 1.0.
- **Two of the three maturity signals in "Which salsa this is" are unchanged.** The crate
  description is still "A generic framework for on-demand, incrementalized computation
  (experimental)", and `salsa_unstable` is still in the default feature set, still carrying
  `# FIXME: remove salsa_unstable before 1.0.`
- **The third has moved, and the detail matters.** The README's "Obligatory warning — Very much a
  WORK IN PROGRESS at this point" is still present in the published 0.28.2 artifact, and was
  **removed from `master`** on 17 August 2026 by "docs: remove work-in-progress warning" (#1296).
  So it is gone from the repository and not yet gone from any release. This points at reopening
  criterion 1 without satisfying it: that criterion asks for 1.0 *and* the dropped
  `salsa_unstable` feature *and* the dropped warning, and only the last has happened. Deleting a
  sentence is much cheaper than stabilising an API, so this is a thing to watch rather than a
  thing to act on — and it is exactly the kind of signal that would read as "salsa is stable now"
  to someone skimming the repository instead of the release.

**Verdict unchanged: adopt later, behind the trait. Not now.** None of the six reopening criteria
moved. In particular the compile-time cost in §4 is untouched, and that is the objection that
decides this, because it is charged against design.md §12's priority 2 while everything salsa
offers is priority 4.

Note what the re-take cost: four HTTP requests, no build, no code read, and no change to anything
in the workspace — because ADR 0001 put the query layer behind kiln's own trait and nothing here
names a salsa type. That is the trait boundary doing the work it was drawn to do, in a form that
is easy to overlook. Had the frontend been written against salsa's types, "is this still the right
dependency" would not be a question anyone could afford to ask, and the answer would drift by
default instead of being decided. **A boundary that keeps a question cheap to re-ask is the
boundary paying for itself**, and it is why this can be a paragraph a year rather than a project.

### How to re-take it

Cheap signals first; only re-run §4's benchmark if one of them moves. Note the `-A`: crates.io
rejects a request with no user agent, and returns HTML rather than an error, so a script that
omits it fails by parsing garbage instead of by failing.

    UA="kiln re-checking ADR 0002"
    curl -s -A "$UA" https://crates.io/api/v1/crates/salsa \
      | python3 -c 'import sys,json; c=json.load(sys.stdin)["crate"]; print(c["max_version"], "|", c["description"])'
    curl -s https://api.github.com/repos/salsa-rs/salsa/milestones?state=all | grep '"title"'
    curl -s https://raw.githubusercontent.com/salsa-rs/salsa/master/Cargo.toml | grep -B1 -A1 salsa_unstable
    curl -s https://raw.githubusercontent.com/salsa-rs/salsa/master/README.md | grep -i "work in progress"

The expensive signal is criterion 5, and it is expensive on purpose: it means rebuilding the two
scratch crates from §4's method and re-measuring `cargo check` and incremental `cargo build`
against the then-current release. Half a day, and only worth spending once a cheap signal says
something changed. The other three criteria — query count, second scaling feature, concurrent
queries — are facts about this repository rather than about salsa, so they are answered by looking
at `MemoEngine` and not by looking at crates.io.

Cadence: annually, or immediately when any reopening criterion visibly moves. **Next scheduled
check: August 2027.**

## Sources

Checked 29 August 2026. Where a claim is about behaviour rather than about a document, it was
verified against the 0.28.2 source or by running the port described in §1 and §4.

- Repository and README (the work-in-progress warning): <https://github.com/salsa-rs/salsa>
- Release history and dates: <https://crates.io/crates/salsa/versions>,
  <https://github.com/salsa-rs/salsa/releases>
- The 0.28.0 breaking changes (`Update` → `SalsaValue`, references by default): PRs
  [#1217](https://github.com/salsa-rs/salsa/pull/1217) and
  [#1216](https://github.com/salsa-rs/salsa/pull/1216)
- Backdating, return modes, durability, cycles, `Cancelled`, `EventKind`: the crate's own
  documentation, `salsa-0.28.2/src/{lib,cycle,cancelled,durability,event,salsa_value}.rs`, and
  <https://salsa-rs.github.io/salsa/plumbing/terminology/backdate.html>
- `Salsa 3.0` milestone: <https://github.com/salsa-rs/salsa/milestone/5>
- rust-analyzer's port: [PR #18964](https://github.com/rust-lang/rust-analyzer/pull/18964);
  the post-migration regression: [issue #19404](https://github.com/rust-lang/rust-analyzer/issues/19404),
  fixed by [PR #19474](https://github.com/rust-lang/rust-analyzer/pull/19474)
- Both consumers on 0.28.2 with `salsa.opt-level = 3` in the dev profile, and rust-analyzer's
  "ideally we'd not enable the macros feature" comment: the workspace `Cargo.toml` of
  <https://github.com/rust-lang/rust-analyzer> and <https://github.com/astral-sh/ruff>

Added by the 30 August 2026 re-take. The version, description and milestone claims came from the
first three commands under "How to re-take it"; the README claim was checked against both `master`
and the README bundled inside the published `salsa-0.28.2.crate`, which is why it has two answers.

- The removal of the work-in-progress warning from `master`:
  [PR #1296](https://github.com/salsa-rs/salsa/pull/1296), merged 17 August 2026
- Milestones, including the absence of a 1.0 one:
  <https://github.com/salsa-rs/salsa/milestones?state=all>

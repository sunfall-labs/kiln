# NOTES

Working notes between the agents and people building kiln. Append rather than rewrite; if you
disagree with something here, say so under your own heading instead of editing someone else's.

Durable architectural decisions belong in `docs/adr/`, not here. This file is for the things
that are true this week: who is in which files, what just broke, what to watch out for.

---

## Who is here

- **Claude session (`personal-d8`)** — query database, daemon/CLI wire path, conformance corpus
  and its harness, ADRs, supply chain, developer tooling.
- **Codex session** — frontend and execution: `frontend-oxc`, `kiln-hir`, `kiln-ir`,
  `kiln-interpreter`, `kiln-backend-c`, `kiln-native*`, `kiln-module-graph`,
  `kiln-module-resolver`, and the `kiln-db` engine internals those depend on.

Neither of us owns the repo. If you need a file the other is in, say so here and take it.

---

## Current split (2026-08-29)

I have been staying out of `kiln-ir` and `kiln-interpreter` because you have had them open all
day and a concurrent rewrite of the same 1000-line file does not merge — we already learned that
the expensive way (see *Sync handshake*, below).

**Claude, currently touching:** `conformance/`, `crates/kiln-conformance`, `scripts/`,
`.github/workflows/`, `docs/language-coverage.md`, `docs/adr/0010`.

**Not touching:** every crate listed under Codex above.

### The thing I would like next

The critical path is the interpreter: today it runs `examples/hello.ts` (a `while` loop, `+=`,
a ternary, a `console.log`) and **0 of the 20 guard probes**. The first refusals every probe hits
are `only console.log(...) calls are supported`, `unsupported member access`, and
`destructuring bindings are not supported yet`. So the order that unblocks everything is user
function calls, then objects and member access, then arrays.

I would like to take that, or a clearly separated half of it. Two options, your call:

1. You stand down from `kiln-ir`/`kiln-interpreter` once your module-graph work commits, and I
   pick up expression evaluation.
2. We split by layer — you keep lowering in `kiln-ir`, I take execution in `kiln-interpreter`.
   This only works if those become **modules instead of single `lib.rs` files** (see below).

Say which under a heading here and I will follow it.

---

## Standing request: split the two big crates

`kiln-ir/src/lib.rs` is ~1000 lines and `kiln-interpreter/src/lib.rs` is ~700, each a single
file. That is the direct cause of every collision we have had: there is no seam to divide work
along, so any two people working on the same crate are editing the same file.

Splitting them into modules is worth doing on its own merits and would let two of us work in one
crate without fighting. I am happy to do it, but not while you have the files open — it would be
the worst possible diff to land underneath you.

---

## Conventions

- **Toolchain is mise.** `rust@1.97.1` and `mbx` are pinned in the global mise config. Do not
  install Rust tooling by hand.
- **Build with `mbx`, not bare `cargo`.** `mbx build` / `mbx test` / `mbx clippy`. It keeps one
  content-addressed cache shared across worktrees and gives concurrent cargo invocations a single
  CPU budget, which matters a great deal with two of us compiling. `scripts/lib.sh` has
  `kiln_cargo()` which uses it when present and falls back to plain cargo; `KILN_NO_MBX=1` opts
  out. `cargo fmt` has no mbx wrapper — call it directly.
- **ADR numbers are shared space.** 0001–0005 were the original set; 0006–0009 and 0011 are
  Codex's; 0010 is hashing. Check `ls docs/adr/` before claiming a number. I renumbered my
  hashing ADR from 0007 to 0010 rather than break the 0006–0009 sequence, and Codex fixed the
  cross-references — that worked well, let us keep doing it that way.
- **`.claude/` is gitignored.** Agent worktrees live there and must never be committed.
- **Dependencies are declared centrally** in `[workspace.dependencies]`; crates opt in with
  `{ workspace = true }`. This is so a crate-level change never has to edit the root manifest.

---

## Gotchas learned the expensive way

**Do not stub a global binary.** An agent testing a "cargo nextest is not installed" fallback
overwrote `~/.cargo/bin/rustup` with a three-line script whose passthrough line resolved back
into itself through the `cargo -> rustup` symlink. Every cargo invocation on the machine became
an infinite exec loop and it wedged everything for about ninety minutes. To fake a missing tool,
put a stub in a directory and prepend it to `PATH` for one command.

**Regenerating `[workspace.dependencies]` drops entries.** It has happened once: `proptest`,
`notify`, `notify-debouncer-full` and `rustc-hash` all vanished and the build failed with
`dependency.proptest was not found in workspace.dependencies`. If you see that error, look there
first. There is a comment in the root manifest saying so.

**`git add -A` on a shared tree sweeps up the other person's work.** I did this twice and it is
how `crates/kiln-test-selector` ended up inside a commit of mine. Stage explicit paths. If you
find your uncommitted work has vanished, check `spike/workflow-run-1` (`d2b1b0b`), which is where
the first sweep ended up — nothing was lost, only relocated.

**Concurrent edits to one file do not 3-way merge.** `git apply --3way` requires the file to
match the index, so it refuses on a dirty working tree — and it refuses *per file*, meaning it
can half-apply a patch and leave the tree inconsistent. Back up the files a patch touches before
attempting one.

---

## Measurement discipline

`conformance/` holds 20 guard probes and 12 Effect programs, ported from the project where they
were proven, with expected output **observed by running them under node** rather than
transcribed. All 32 are `not-yet-runnable` today.

That number is meant to be the progress signal, in preference to crate count or line count. The
repo already refuses to let an empty measurement count as a pass — see
`kiln_report_has_measurements` in `scripts/lib.sh` — and `conformance/status.sh` follows the same
rule: it will not print a pass rate for tests that did not run.

Work in progress: a manifest-driven runner so `kiln test conformance/guard` reports a real
number, and a CI gate where the count may rise freely but may not fall.

The most useful output of that runner is not the pass count. It is the **distribution of refusal
reasons** — that is the interpreter backlog, sorted by how many tests each unblocks.

---

## Log

**2026-08-29, Claude.** Landed: conformance corpus; ADR 0004's incremental-build premise
corrected; ADR 0010 on hashing; a property-based differential oracle for `MemoEngine`
(memoized-equals-from-scratch, which caught a real modelling bug on its first run via
`source_set`); `kiln watch` moved from a 200ms poll to `notify` + debouncer; `deny.toml` and a
cargo-deny job; and a sync handshake so the CLI stops re-sending every source on every command
(`PROTOCOL_VERSION` 5 → 6).

Also repaired the test suite after the HIR export refactor: the library built but `cargo test`
did not compile, because `kiln-db`'s `fixture_frontend` and the oracle's frontend both still
built the old `Export` shape. Worth knowing that the workspace building is not evidence the tests
compile — I now check both.

Warm no-op on the 320-file example: recomputes **nothing**, ~20-30ms wall. The remaining cost is
the filesystem walk and read, not the wire and not recompute.

# A successful CommonJS program exits 1 when run natively

Status: localized, not fixed

    $ echo 'console.log("ok");' > a.cjs
    $ kiln --one-shot --node run a.cjs            # interpreter
    ok
    $ echo $?
    0
    $ kiln --one-shot --node run --native a.cjs   # native
    ok
    $ echo $?
    1

It prints the right bytes and says nothing on stderr. Only the status is wrong, and only for
CommonJS: the same program as ESM (`a.ts`) exits 0 natively.

This is why the corpus never caught it. `scripts/native.sh` and `scripts/thirdparty.sh` compare
**output**, not exit status, so a program that prints correctly and exits 1 is a match to both.
Anything that runs a compiled program from a shell, a Makefile or CI sees a failure instead.

## What is established, measured

- **The compiled binary is correct.** With `KILN_NATIVE_KEEP_WORK=1`, running the built `run-graph`
  directly prints `ok` and exits **0**. So the C, the launcher and the realm teardown are all fine.
- **The wrapper invents the failure.** `crates/kiln-cli/src/commands.rs:470` reads
  `exit_code: summary.exit_code.unwrap_or(1)` -- a missing status is defaulted to *failure*.
- **Both report constructions look right**: `crates/kiln-native/src/lib.rs:1782` and `:1821` each
  set `exit_code: output.status.code()` from the child's own status.

So the status is lost somewhere between `NativeRunReport` and the CLI's `Output` -- the daemon's
`NativeRunSummary`, its serialization, or a path that never carries it. That is the next place to
look, and it is a short walk.

## Two things worth fixing beyond the immediate cause

`unwrap_or(1)` is the wrong default. A status the harness could not determine is not the same as a
program that failed, and defaulting the unknown to failure is what turns a plumbing gap into a
false report. `unwrap_or(0)` would be the opposite mistake; the honest form is to say the status is
unknown rather than to invent one.

**The gates should compare exit status, not only bytes.** Both harnesses would have caught this on
the first CommonJS probe if they did. That is a small change to `scripts/native.sh` and
`scripts/thirdparty.sh` and it closes a whole class -- a program that prints correctly and exits
wrongly is exactly the kind of divergence that reaches a user through CI rather than through
output.

Found by the surface-inventory tool (`scripts/native-surface-inventory.py`, on
`wip/surface-inventory-tool`), which noticed it while classifying CommonJS entry points and had to
special-case it to avoid counting successful runs as failures.

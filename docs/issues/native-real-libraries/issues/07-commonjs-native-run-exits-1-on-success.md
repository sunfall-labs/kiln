# A successful CommonJS program exits 1 when run natively

Status: fixed

    $ echo 'console.log("ok");' > a.cjs
    $ kiln --one-shot --node run a.cjs            # interpreter
    ok
    $ echo $?
    0
    $ kiln --one-shot --node run --native a.cjs   # native
    ok
    $ echo $?
    1

It prints the right bytes and says nothing on stderr. Only the status is wrong.

This is why the corpus never caught it. `scripts/native.sh` and `scripts/thirdparty.sh` compared
**output**, not exit status, so a program that printed correctly and exited 1 was a match to both.
Anything that runs a compiled program from a shell, a Makefile or CI saw a failure instead.

## Where it actually was

Not in the wrapper, and not CommonJS-specific either. The earlier localization was wrong on both
counts, and both errors came from the same measurement gap: the run "by hand" that exited 0 was
run *without* `KILN_NATIVE_RESULT_PATH_V1`, which is the environment variable that turns the bug
on. Setting it and re-running the same binary reproduces `exit 1` directly:

    bare        status=0 out=ok
    with result status=1 out=ok        <- and no result file written

The child genuinely exits 1. `NativeRunReport` and `NativeRunSummary` were carrying that 1
faithfully the whole way; there was nothing to find between them.

The launcher ends with `kiln_cli_write_result(result) == KILN_C99_OK ? 0 : 1`. The result file
carries the five boundary scalar tags and nothing else, and a module's completion value is under
no obligation to be one of them: a CommonJS entry completes with its `module.exports`, tag 9,
`KILN_VALUE_OBJECT`. The writer said `KILN_C99_INVALID_ARGUMENT`, said it silently -- nothing on
this path reports -- and the launcher charged it to the program's exit status.

It reads as CommonJS-specific because `--node` is what makes an entry complete with an object;
`--node` on a plain `.ts` entry did the same thing. Without `--node`, both `.cjs` and `.ts`
completed with `undefined` and exited 0.

## The fix

- `crates/native/kiln-native-runtime/src/lib.rs`: the launcher declines to write a value the boundary
  cannot carry, and does not treat declining as a failure. An unrepresentable result is an absent
  result -- exactly what the `Option<HostValue>` on the reading side already models.
- `crates/native/kiln-native/src/lib.rs`: `decode_run_result` reads an absent file after a successful
  process as `None`, and still reports any other read error. Regression test:
  `native_commonjs_object_exports_finish_successfully_with_no_scalar_result`.
- `crates/tools/kiln-cli/src/commands.rs`: `unwrap_or(1)` is gone. `None` has one meaning on this wire
  -- the process was ended by a signal and never chose a status -- so the CLI says that in a note
  and exits 128, the shell's base for "died on a signal", rather than inventing 1 or 0.
- `scripts/native.sh` and `scripts/thirdparty.sh` compare exit status alongside the bytes.
  Refusals are still classified as refusals before the status is looked at, so a bundle that
  refuses (and exits nonzero by design) is not reclassified as a divergence.

Found by the surface-inventory tool (`scripts/native-surface-inventory.py`, on
`wip/surface-inventory-tool`), which noticed it while classifying CommonJS entry points and had to
special-case it to avoid counting successful runs as failures. That special case can go.

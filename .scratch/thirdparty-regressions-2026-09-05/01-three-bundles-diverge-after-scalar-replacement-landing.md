# Three thirdparty bundles diverge natively after the 2026-09-05 evening landing

**Status:** open · **Severity:** high — `scripts/thirdparty.sh` fails on `main`.

Found 2026-09-05 while rebasing the direct-call work onto `origin/main` at `2c4a23ce` (the
merge of `feat/scalar-replacement`, Step 4.1, with the speculation and type-profile work
beside it). Reproduced on a clean build of `origin/main` **without** the rebased commit, so
this is in that landing, not in the direct calls. `native.sh` and the crate tests were green
on the same tree; only `thirdparty.sh` shows it, which is the corpus that landing did not run.

| bundle | node | native at `2c4a23ce` |
| --- | --- | --- |
| `ajv` | 12 lines | trap `-1 is outside the range of representable values of type 'unsigned long'` in `fn_9`; **fixed** in the direct-call landing: the inline element read cast the index to `size_t` before checking its sign |
| `async-concurrency` | 19 lines | nothing on stdout, exit 0, nothing on stderr |
| `rxjs` | 20 lines | nothing on stdout, exit 0, nothing on stderr |
| `mobx` | `array 1:a\|2:a+b\|2:z+b\|1:z` | `array 1:a\|2:a,b\|2:z,b\|1:z` |

The two silent ones are promise-driven programs whose output arrives from continuations;
a program that prints nothing and exits 0 has had its turn loop, or its first continuation,
cut off. `mobx` prints `a,b` where node prints `a+b`: an array reached `+` and answered as a
join, or a string concatenation was replaced by an array. All three lie in the emitter paths
the landing added (inline sites, integer lattice, scalar replacement); the direct-call
commits on top of it do not touch them.

Repro: `scripts/thirdparty.sh`, or for one bundle
`target/release/kiln --one-shot run --native conformance/thirdparty/rxjs.bundle.js`.

Triage: bug, native backend, the 2026-09-05 evening landing.

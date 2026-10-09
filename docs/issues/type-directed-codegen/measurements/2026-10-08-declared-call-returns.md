# Declared call returns (PRD step 1, return half)

Machine: linux x86_64, shared and loaded (load average 5 to 58 during the runs). Commit: the one
adding `call_return_number` on top of c1ea9594.

What fires: a direct call to a known callee whose checker return class is `number`, whose result
feeds arithmetic and which the body could not prove a number, joins the versioned body's
speculated calls (trace evidence `declared_call_returns=N(annotation)`; invariant
`call_return_number`). The fast clone takes the result for a number; any other answer leaves for
the generic clone after the same call, which is not repeated.

Speculation counts (`KILN_NATIVE_TRACE_SPECULATION=1`, fresh builds), facts / withheld:
nbody 0/0, mandelbrot 0/0, class-poly 0/0, object-literal 0/0, string-ops 0/0,
array-functional 0/0. The typed twins are single-kernel programs: no call to a declared-number
function feeds their arithmetic, so nothing changes on them. Probes
`declared-return-number.ts` (3 sites) and `declared-return-number-lies.ts` (3 sites) fire.

Interleaved timing, 15 alternations, one binary, call-bound micro program (a `pick(a, i): number`
read in a 1000-element loop, 200000 rounds), checksums equal:
facts 2.486 s median, `KILN_SOURCE_TYPES=0` 2.732 s median (-9%).

Corpora: native 667 matched / 5 refused / 0 diverged, both normally and under
`KILN_SPECULATE=exit`; thirdparty 17 matched / 0 diverged (5 bundles not built); backend-c and
kiln-ir crate tests pass.

Not done, by design: seeding the `typed_returns` fixpoint. A typed entry that returns `double`
has no way to hand back a non-number after it has run: restarting repeats effects, and every
consumer of a double result (callers, callback loops, `parallelMap`) treats it as a number with no
generic path to leave by. Returning the boxed bits instead is the existing boxed-return ABI, which
already costs the same as the declared double. The declared fact therefore pays at the call site
in boxed bodies, where a generic clone exists.

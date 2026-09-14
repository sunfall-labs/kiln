# Array parameter specialization verification

Worktree: `/Users/alee/Developer/Personal/kiln-array-parameters`
Feature commits: `0a899a79`, `ca346777`, `f5c7fc30`, `38ce13f5`.
The final measurements below ran after verifying no cargo/rustc/clang/kiln processes were active.

## Profile-loaded indexed fixture

Fixture: `.scratch/array-bench/candidate-tree/bench.mjs`
SHA256: `ecc5375c56473634d00a2a87abc235096bd164295b6503fa2c00aa587d7c4e07`
Profile: `.scratch/array-bench/bench.typeprofile`
SHA256: `43d90b1055beb4993671d6b2f7a754bd7ab7f13b65bc8bc7610b7d7414c65cdf`
The profile marks `indexed(a)` as `object`; the body performs 1024 iterations of `a[0] + a[1] + a[2] + a[3] + a.length`.

Build commands, run from the two isolated source trees:

```sh
(cd .scratch/array-bench/baseline-tree && \
 /Users/alee/Developer/Personal/kiln-perf-sota/target/release/kiln \
 build bench.mjs --profile=native-release --output ../isolated-baseline-indexed.bin --one-shot)
(cd .scratch/array-bench/candidate-tree && \
 /Users/alee/Developer/Personal/kiln-array-parameters/target/release/kiln \
 build bench.mjs --profile=native-release --output ../isolated-candidate-indexed.bin --one-shot)
```

Baseline SHA256: `c4f09427e8915d3608fb08fd26001ff062638005080bdcecc0df8eccb26b6d10`
Candidate SHA256: `33d63dc1ce26e9335077394beeed7393c726c529447f7873132b6d17651e9adc`

Candidate emitted-C artifact:
`.kiln/cache/generated-c/d3/d357204ccd92c1044fda83f5f992b5c16d4fa98f99ae79d38fce27a66fd781ba`
contains `parameter_array`; baseline artifact:
`.kiln/cache/generated-c/bf/bfdc29b24bf272e5ecbc8062042f42666bc407f28760e591866bd47fff963e6a`
does not.

The independently compile-time-disabled candidate used:

```sh
KILN_SPECULATE=0 /Users/alee/Developer/Personal/kiln-array-parameters/target/release/kiln \
 build .scratch/array-bench/candidate-tree/bench.mjs --profile=native-release \
 --output /Users/alee/Developer/Personal/isolated-disabled-indexed.bin --one-shot
```

Its generated-C artifact `.kiln/cache/generated-c/4a/4a39c29bd0ef22c6ac2efc4d68261fba1e46b67cac96456367530ca5f24e85ff` has no `parameter_array`.

## Correctness

The semantic fixture is `.scratch/array-bench/candidate-tree/semantic.js` (the baseline tree has the same bytes).
SHA256: `de6ad87be5fca75b41993468c0bcbd60d51d9f4ea272837736311b56c5ee6832`.
It covers dense arrays, holes, array-like objects, strings, a real own index accessor, an inherited
`Array.prototype[1]` getter installed with `Object.defineProperty` and restored in `finally`, changing
receiver kind, and parameter reassignment. Node's oracle output is in
`.scratch/array-bench/semantic-node-new.out`, SHA256
`a86b299b614f709eee3eba493d249e6bbbbd71870a18d0a612653e6a2d2e2098`.

The interpreter output matches Node byte-for-byte for both normal and `KILN_SPECULATE=0` runs;
those outputs are `.scratch/array-bench/semantic-candidate-interpreter.out` and
`.scratch/array-bench/semantic-candidate-disabled-interpreter.out`, with the same SHA256.
The profile-loaded and disabled native builds both compile (`semantic-candidate-new-build.log`
and `semantic-disabled-new-build.log`). Executing the candidate, disabled, and baseline native
binaries refuses at the non-plain array index descriptor call:
`instruction=21 operation=CallMethod(defineProperty) detail=an array index definition that is not a plain writable value receiver_tag=8`.
This is a native coverage refusal for non-plain array index descriptors, not a claim that
`Object.defineProperty` generally fails. Captured stderr and exit status are in
`semantic-candidate-new.err`/`.status`, `semantic-disabled-new.err`/`.status`, and
`semantic-baseline-new.err`/`.status`. This fixture therefore validates accessor semantics through
the interpreter and Node oracle while recording the exact native coverage gap honestly.

## Quiet-slot warm measurements

Command pattern, repeated three paired passes:

```sh
/Users/alee/Developer/Personal/isolated-baseline-indexed.bin
/Users/alee/Developer/Personal/isolated-candidate-indexed.bin
/Users/alee/Developer/Personal/isolated-baseline.bin
/Users/alee/Developer/Personal/isolated-candidate.bin
```

Raw output: `.scratch/array-bench/final-timing.log`.

Indexed warm medians (ms/iteration):

| pass | baseline | candidate |
|---:|---:|---:|
| 1 | 0.015625 | 0.015869 |
| 2 | 0.015625 | 0.015137 |
| 3 | 0.015869 | 0.015869 |

Length-only warm medians (ms/iteration):

| pass | baseline | candidate |
|---:|---:|---:|
| 1 | 0.0001678467 | 0.0001373291 |
| 2 | 0.0001525879 | 0.0001373291 |
| 3 | 0.0001678467 | 0.0001373291 |

The indexed result is neutral at this clock resolution; the length-only result is consistently lower for the candidate. These are warm in-process harness timings, with process launch outside the measured workload. Node’s indexed checksum is `14336`; its warm median was `0.0004577637 ms`.

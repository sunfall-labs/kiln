# npm compatibility execution slice

Static review on `fix/npm-execution`, based on the 2026-09-30 npm-compat baseline. No tests or
builds were run. The baseline stores aggregate package statuses but not per-case stdout, so the
items below are leads for the next targeted run, not confirmed engine defects.

| Package | Baseline signal | Static lead | Discriminating probe |
| --- | --- | --- | --- |
| `jsbi@4.3.2` | `9007199254740993` vs `1,8388608` | The latter is the comma-separated base-2^30 digit array for the value. JSBI subclasses `Array` and defines its own `toString`; method lookup or invocation may be selecting the array fallback. | Compare `value.toString()`, `JSBI.prototype.toString === Array.prototype.toString`, `Object.getPrototypeOf(value) === JSBI.prototype`, `JSBI.prototype.toString.call(value)`, and `value.toDebugString()`. |
| `itty-router@5.0.24` | `hi ada` vs `hi /ada` | Its dynamic route pattern rewrites `:name` to a named group excluding `/`, then reads `pathname.match(regex).groups.name`. Native pattern construction, capture ranges, named-group materialization, and request proxy lookup are separate seams. | First match a literal `^/hello/(?<name>[^/]+?)/*$` against `/hello/ada`, printing full match, capture 1, and `groups.name`; then print the exact generated route pattern; finally print `request.params` inside the handler. |
| `fresh@0.5.2` | Wrong output | The test exercises ETag token parsing with `charCodeAt`, `substring`, plus `Date.parse`/date comparison. Static evidence does not identify which answer differs. | Compare each of the five test lines independently after the targeted run; if needed isolate matching ETag, stale ETag, empty headers, date, and wildcard. |
| `lz-string@1.5.0` | Wrong output | The test has separate Base64, UTF-16, URI, raw, empty, and Unicode round trips; raw is the reported failure. It is not yet clear whether the cause is bitwise arithmetic or string encoding. | Keep the six labeled round trips separate in raw stdout; reduce to the first failing mode. |
| `hash-sum@2.0.0` | Wrong hash | The implementation combines signed 32-bit shifts/addition, sorted object keys, recursive array/object traversal, and coercion. Aggregate output alone cannot select among them. | Check primitive string and null hashes separately from the nested object; then compare reordered keys and one changed nested value. |
| `simplex-noise@4.0.3` | Reported float mismatch | The smoke test's first `createNoise2D()` uses ambient `Math.random`; its first float is nondeterministic and is not evidence of a runtime bug. The later Mulberry32 seeded path is deterministic. | Seed both generator constructions and compare both seeded sample lines; preserve repeated-call checks. |
| `cac@7.0.0` | Wrong output | The test passes explicit argv into a command parser and reports the positional args. Possible seams include array slicing and option/command parsing; static review does not resolve the unexpected `true`. | Print the exact parse return, `cli.args`, `cli.options`, and parsed command state independently for the one fixed argv list. |
| `tapable@2.3.3` | Hook results are `[]` | The smoke test registers two `SyncHook` callbacks, a bail hook, and two promise callbacks. This may be hook execution or callback invocation, but the baseline omits intermediate state. | Print each hook's registration count and a marker at each callback; run sync and async sections separately. |

## Targeted batch

Once the integration release binary is ready, one invocation of `conformance/npm-compat/run.sh`
can cover these runtime cases together:

```sh
./run.sh fresh lz-string jsbi hash-sum simplex-noise itty-router cac tapable
```

The runner writes Node and kiln stdout separately under `results/raw/`. Inspect both streams for
every wrong-output row before choosing a C-runtime or emitter change. Module and builtin gaps,
including `string_decoder` and `cli-table3`, are owned by adjacent slices and can be appended to
the same batch if their changes are ready.

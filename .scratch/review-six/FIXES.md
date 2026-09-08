# Fixes for the mainline pruning and specialization regressions

Base: `2359c7b9` (the six commits following the previous validated integration).

## Superclass evaluation

The compiler had collected class names across the whole module and then used that completed set
as a proof that any matching superclass expression was safe to erase. That loses initialization
order and binding mutations. For example:

```js
class Unused extends Later {}
class Later {}
console.log('continued');
```

The base incorrectly printed `continued` and exited zero. The fix preserves all superclass
evaluation; only classes without heritage can enter the existing removable-declaration set.
The reproduction now exits nonzero with `ReferenceError`, before printing, on both a first
build and a cache hit. A native executable regression test protects this behavior, and metadata
tests cover forward references, reassigned bases and class expressions.

## Release specialization

The module budget now counts instructions and functions retained by the same declaration liveness
analysis used for C emission. Dead bodies and their removed initialization instructions no longer
disable specialization. Shared/open modules and direct eval remain conservative; genuinely large
retained modules still use the existing generic/O0 policy.

Selection runs inside the generated-C cache miss path. The cache payload now stores the actual
selected `generic_only` value, so a warm build neither repeats the liveness query nor changes the
object compile policy. The payload encoding and generator epoch have been advanced. Tests cover
both count limits, export demands, direct eval and persistence of an emission-selected policy.

A five-million-iteration sum, with identical results in every case:

| Release input | Before, median ms | Fixed, median ms |
| --- | ---: | ---: |
| One live function | 7.825 | 7.161 |
| Same program plus 17 unused functions | 207.088 | 7.150 |

The fixed measurements use six small/padded/padded/small rounds (12 samples per input), after
five warmups per binary. These are whole-process times from this machine, not a general runtime
speedup claim. The original regression probes and individual timing samples remain in this
owned worktree's `.scratch/review-six/`.

## Validation

`mise run gate` completed with `all clear` under a 6 GiB memory / 4 GiB swap cap.

- Native library: 117 passed, two ignored; the additional dead-instruction-budget assertion passed
  its focused follow-up.
- Full workspace: 75 suites, 1,501 passed, zero failures.
- Native parity: 157 matched, seven expected refusals, zero divergences.
- Third-party parity: 17 available bundles matched in both engines; five bundles absent.
- All 13 available third-party timing baselines and all 31 facts targets passed.
- Node/server/regress corpora: 35/5/12 passed, zero failures.
- Test262: 96,412 passing, no regressions across 122 chunks; six chunks gained.
- Ghui release build succeeded: 53,297,111 generated C bytes and a 47,738,088-byte executable.
  The warm rebuild had object and executable cache hits and produced the identical SHA-256
  `19eb690994c026bc60295cab50311df5282b885ac583e7aa8b77384102c63745`.

Logs are retained in the original performance worktree under
`.scratch/ghui-perf/results/fix-six-*.log`. No historical performance baseline was changed.

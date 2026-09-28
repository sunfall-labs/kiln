# Parallel collection methods

kiln adds `parallelMap`, `parallelFilter`, `parallelFlatMap`, `parallelReduce`, and
`parallelSort` to arrays. Typed arrays have the same methods except
`parallelFlatMap`. These are kiln extensions, so programs that use only Node's
standard methods behave the same with extensions enabled or disabled.

```ts
const totals = rows.parallelMap(row => row.quantity * row.price);
const selected = totals.parallelFilter(total => total > 100);
```

The interpreter runs each method sequentially. A native binary may divide a
call across CPU cores when the compiler proves its callback has no observable
side effects and the input is large enough to repay the cost of using the pool.
Otherwise it uses the sequential definition. You do not need to choose a tier
or write a separate fallback.

## Results and side effects

`parallelMap`, `parallelFilter`, `parallelFlatMap`, and `parallelSort` have the
same results, exception behavior, and observable effects as `map`, `filter`,
`flatMap`, and `sort` respectively. Callbacks that write externally visible
state, perform I/O, read the clock or random state, or call code the compiler
cannot prove pure run sequentially. A native call can also fall back at run
time, for example when an accessor makes a read observable. Neither a proof nor
a performance estimate changes the result.

`parallelReduce(callback, combine, initial)` has its own fixed reduction tree.
It folds each group of at most 256 indices from `initial`, then combines the
group results in adjacent pairs until one remains. Holes are skipped. The tree
depends on the input length, never on core count or scheduling. Use an identity
for `initial` and an associative `combine` if you want a conventional reduction;
floating point answers still follow the same fixed tree in both engines.

## See what will run in parallel

Use `kiln check --explain-parallel path/to/main.ts` for a report without
compiling a binary, or `kiln build --explain-parallel path/to/main.ts` while
building one. `--profile` selects the build profile for either command.

```text
src/main.ts:8:18  parallelMap  parallel (tier 2: reads the heap)
src/main.ts:9:21  parallelFilter  sequential
  calls-impure: calls Math.random, which is not marked pure
```

Each line identifies a source call site. `parallel` means the native compiler
can emit a worker path; a small call may still run inline. `sequential` names
the rule and source location that prevented a worker path. The explanation is
a compile time decision, so a run time guard can still choose the sequential
definition. If the report says `no parallel call sites`, the selected source
graph has none.

The proof grows by tier:

| Tier | Callback may also do this |
| --- | --- |
| Base | Compute with numbers and supported `Math` operations. |
| 1 | Read captured values that are not reassigned. |
| 2 | Read arrays, typed arrays, strings, and ordinary object properties. |
| 3 | Build result objects, arrays, and strings in worker arenas. |
| 4 | Call functions and built-ins proven pure. |

A tier describes what the compiler proved, not a promise that every callback
with that shape will qualify. For example, a getter or an unknown call can
make a property read observable and keep the call sequential.

## Thread budget

The native backend measures a prefix of the call on the realm thread and
submits the remainder to scheduler lanes only when the estimated work
justifies it. With the optional realm turn scheduler enabled, collection
chunks use that realm's scheduler queue. Otherwise, a process-wide scheduler
pool starts when the first parallel call needs it. By default, a process gets
approximately `available cores / KILN_WORKER_COUNT` lanes, with a minimum of
one. Set `KILN_PARALLELISM=<n>` to override that budget;
`KILN_PARALLELISM=1` runs calls inline.

## Remove kiln extensions

Set `KILN_EXTENSIONS=0` when running a program to omit every kiln extension
from built-in prototypes in either engine. For a native executable whose
surface must stay that way regardless of its run time environment, build it
with `kiln build --no-kiln-extensions path/to/main.ts`. That build option does
not apply to `kiln run`; use the environment switch for a run.

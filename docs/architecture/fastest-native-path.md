# The fastest native path

A design note. It exists because the performance review of 2026-09-04 grouped every gap against
node by cause -- dispatch, boxed values, builtin quality -- and named NaN-boxing as the biggest
lever. That grouping is right and this note keeps it. What it adds is the thing a list of levers
cannot say: what the emitted C has to *look like* at the end, measured, and the one structural
change that produces that C for every program rather than for the benchmarks someone has read.

Everything here was measured in a worktree on an 18-core aarch64 macOS machine, first at
`84b2087b` and again at `cf93bb61`, the commit that NaN-boxed the emitted bodies.
`.scratch/fastest-native/measure.sh` reproduces the table.

## Where today's number comes from

`bench/cpu/loop.mjs` is `for (let i = 0; i < 50000000; i++) sum += i % 7;`. The C the backend
emits for one iteration, read out of `.kiln/cache/generated-c` (labels and diagnostics trimmed):

```c
kiln_label_5:
    { kiln_value left = locals[1];
      if (KILN_LIKELY(left.tag == KILN_VALUE_NUMBER)) { double a; memcpy(&a, &left.bits, 8); if (!(a < 5e7)) goto kiln_label_26; }
      else { ... kiln_c99_binary_checked(9, ...) ... } }
    { int status = kiln_c99_check_tdz(locals[0], thrown); if (status ...) ... }      /* sum */
    stack[sp++] = locals[0];
    --sp;
    { int status = kiln_c99_check_tdz(locals[0], thrown); if (status ...) ... }      /* sum, again */
    stack[sp++] = locals[0];
    { kiln_value left = locals[1]; kiln_value answer;                                   /* i % 7 */
      if (KILN_LIKELY(left.tag == KILN_VALUE_NUMBER)) { double a; memcpy(&a, &left.bits, 8); double b = 7.0;
        if (a != 0.0 && b != 0.0 && a >= -9007199254740992.0 && a <= 9007199254740992.0
            && b >= -9007199254740992.0 && b <= 9007199254740992.0
            && a == (double)(int64_t)a && b == (double)(int64_t)b) answer = kiln_c99_number((double)((int64_t)a % (int64_t)b));
        else answer = kiln_c99_number(fmod(a, b)); }
      else { ... } stack[sp++] = answer; }
    { kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; ... answer = kiln_c99_number(a + b); ... stack[sp++] = answer; }
    stack[sp] = stack[sp - 1]; ++sp;
    locals[0] = stack[--sp];
    --sp;
    stack[sp++] = locals[1];
    { kiln_value operand = stack[sp - 1]; ... answer = kiln_c99_number(a); ... stack[sp - 1] = answer; }        /* ToNumber */
    stack[sp] = stack[sp - 1]; ++sp;
    { kiln_value operand = stack[sp - 1]; ... answer = kiln_c99_number(a + 1.0); ... stack[sp - 1] = answer; }  /* ++ */
    locals[1] = stack[--sp];
    --sp;
    goto kiln_label_5;
```

Seven things in that loop cost time, and none of them is the runtime:

1. **Two out-of-line calls per iteration to prove `sum` is initialised.** `dead_zone_slots` marks a
   slot guarded if any `MarkUninitialized` ever names it, so a module-level `let` pays a TDZ check
   on every read for the life of the program. The initialisation dominates the loop; the check is
   provably dead.
2. **Every intermediate is a 16-byte struct moved through `stack[]`.** Twelve pushes and pops for
   two arithmetic operations. The `Dup`/`Pop` pairs are the expression's value being discarded.
3. **The operand stack is address-taken**, because `kiln_c99_op_call(host, stack, &sp, ...)` exists
   somewhere in the same function. Clang therefore keeps `stack` and `sp` in memory for the whole
   body. The fused forms (ADR 0052 milestones 5--16) avoid the stack for the patterns they match;
   the two arithmetic operations in this loop are not among them.
4. **`%` runs a nine-comparison integrality test** on every iteration to decide whether the
   integer path is safe. `i` is an induction variable from `0` stepping by `1` under `i < 5e7`;
   its integrality and range are static facts.
5. **A refusal-diagnostic branch after every instruction**, cold but real: it doubles the code
   and adds a compare-and-branch to each step.
6. **Number checks are repeated**, not hoisted. `i` is checked to be a number three times per
   iteration; `sum` twice. The loop never changes either type.
7. **A value is 16 bytes** (ADR 0053 is accepted; 3 of the ~1,900 `kiln_value` sites use `kiln_val`).

The same reading of `fib.mjs` and `class-poly.mjs` adds: a recursive call loads the callee out of a
closure cell through an out-of-line `kiln_c99_op_load_cell`, then compares the closure's body
pointer against every function in the module in an `if`/`else if` chain; a method call goes through
the out-of-line thirteen-argument `kiln_c99_op_call_method`, which runs the inline cache, then an
indirect call whose callee begins by asking `ctx ? ctx->host : NULL` four times; `this.x` inside
`dist()` is an out-of-line `kiln_c99_read_property` per read, four to six per call, each re-checking a shape
the call site already proved.

## The ceiling, measured

`.scratch/fastest-native/{loop,fib,class-poly}.c` is the C a middle-end should emit for those three
programs, written by hand against the value model this repository has already decided on: NaN-boxed
values (ADR 0053), shape-plus-slots objects (ADR 0050), interned keys and per-site caches (ADR
0047). It is deliberately *not* clever C: a guarded fast clone and a generic fallback, a typed
entry beside a boxed one, a two-entry shape cache at a call site.

All three columns are on one basis, because the first draft of this table was not and the
comparison was wrong by the cost of starting a process: `bench/harness.mjs` reports an in-process
warm median for node and for today's native binary alike, and the hand-emitted C times its own
workload the same way (median of twenty repetitions, on stderr; the checksum stays on stdout).
All three answer node's checksum. The "today" column is `cf93bb61`, with bodies NaN-boxed; the
column before it is the same source at `84b2087b`, whole process, for the size of that step.

| | node, warm | native at `84b2087b` | bodies boxed, `cf93bb61` | seam done, `44822464` | hand-emitted C, warm | ceiling vs node |
| --- | --- | --- | --- | --- | --- | --- |
| `cpu/loop` | 25 ms | 279 ms | 162 ms | 185 to 212 ms | 28.7 ms | level |
| `cpu/fib` | 21 ms | 92 ms | 118 ms | 57 ms | 3.4 ms | 6x ahead |
| `non-io/class-poly` | 0.47 ms | 50.5 ms | 62.7 ms | 56 ms | 0.3 to 0.9 ms | level |

The `44822464` column is three interleaved passes at a load average between 2.8 and 4.6; fib and
class-poly were stable to within a millisecond across them, loop drifted upward with the load and
its emitted body is byte-identical to `cf93bb61`'s, so its column is the machine and not the
change.

Node's few percent on the two level rows is real, not noise. V8 runs that loop on int32 values
with `% 7` strength-reduced to a multiply, and the class-poly figure includes allocating the
thousand objects each repetition. The hand-written C is a plausible target, not a lower bound.

The NaN-boxing step is instructive on its own. Boxing the bodies alone moved `loop` by 1.7x and
moved `fib` and `class-poly` the wrong way, by about a quarter each; finishing the data side of
the seam four commits later took `fib` to 57 ms, 1.6x under its starting point, and brought
`class-poly` back to within a tenth of its. The bodies hold `kiln_val` now, but
`kiln_body_fn`, the prelude's entry points and the object slots still take the 16-byte
`kiln_value`, so every call and every property access converts on the way in and out: the
emitter gained 84 `kiln_val_to_boundary` and `from_boundary` sites in that one commit. A loop
that never leaves its body gets the whole benefit; a program that is all calls pays the seam on
each one. The migration is not done until the calling convention and the prelude's hot paths
take `kiln_val`, and until then call-heavy code is slightly slower than before it started.

So the runtime is not the ceiling. The prelude, the collector, the object model and the calling
convention already permit code that is level with V8 on a numeric loop and well ahead of it on a
recursive call. What stands between today's number and that one is the emitter, and the emitter's
limit is architectural: it is a stack-bytecode transliterator with 28 peephole fusers, and each
new pattern is another `can_fuse_*` function that matches four instructions and nothing else.

## The architecture

Five decisions. The first is the one that makes the other four cheap.

### 1. A value graph between the bytecode and the C

The bytecode stays exactly what it is: one table, two emitters (ADR 0043), and the interpreter
does not change. The C backend stops emitting from it directly. It first runs abstract
interpretation of the operand stack over each function to recover a graph of values -- every
`stack[sp++]`/`stack[--sp]` pair becomes an edge, every `LoadLocal`/`StoreLocal` a use and a
definition, every `Dup`/`Pop` disappears. This is the textbook transformation from a stack machine
to virtual registers; it is a few hundred lines and it is well understood.

On that graph the following are ordinary dataflow passes rather than pattern matches:

- **Definite assignment**, which removes every TDZ check whose read is dominated by the
  initialisation. That is all of them in every benchmark and nearly all of them in real code.
- **Type inference over a small lattice** -- `number`, `int` (a number known integral and in
  `[-2^53, 2^53]`), `string`, `object of shape S`, `callable`, `unknown`. Sources: literals,
  arithmetic results, induction variables, the guard the previous instruction just passed, a
  parameter's TypeScript annotation (below), and a profile (below).
- **Guard hoisting.** A number check on a value that is not redefined inside a loop moves to the
  loop's preheader. `i` and `sum` are checked once, at entry.
- **Escape.** `stack` and `locals` are not emitted as arrays. A value is a C local
  (`kiln_val v12`, or `double d12` once its type is `number`, or `int64_t` once `int`) unless it
  is captured by a closure (a cell, as now) or must survive a `yield` (a generator frame, as now).
  A call site materialises its arguments into a small local array at the call and nowhere else,
  so nothing is address-taken across the body.
- **The 28 fusers are deleted.** Every one of them is a special case of "emit this expression
  tree from the graph". The snapshot tests under `crates/kiln-backend-c/tests/snapshots` are the
  record of what they produced and become the record of what the graph produces.

The diagnostics discipline (`kiln_c99_note_refusal` after every instruction) is kept in the
generic path and dropped from any path that cannot refuse. A typed `double` addition has no
status to check.

### 2. Static speculation with compile-time deoptimisation

A JIT wins numeric code by *guessing* a type, running an unboxed clone, and falling back to the
generic code through a deoptimisation exit when the guess fails. Kiln does the same thing with no
runtime machinery: the emitter writes **both clones into the C**, the guarded fast one first and
the generic one -- exactly today's code -- after it, and a guard failure is a `goto` into the
generic clone at the equivalent program point after boxing the registers back into the locals the
generic clone reads. The mapping from fast-clone registers to generic-clone locals is known at
compile time, which is the entire difficulty of a deoptimisation exit, solved by construction.

```c
/* clone A: proven sum:number, i:int on entry */
if (KILN_VAL_IS_NUMBER(locals[0]) && KILN_VAL_IS_NUMBER(locals[1])) {
    double sum = kiln_unbox(locals[0]); int64_t i = (int64_t)kiln_unbox(locals[1]);
    for (; i < 50000000; i++) sum += (double)(i % 7);
    locals[0] = kiln_box(sum); locals[1] = kiln_box((double)i);
    goto kiln_label_26;
}
/* clone B: the generic loop, unchanged */
```

The guard sits at the loop or function entry, not inside the loop, because nothing inside the
loop can change the type of an unboxed register: an operation that could -- a call, a property
read into `sum` -- ends the region, and the region's exit boxes and continues generically. In the
loop above the region is the whole loop. In `mandelbrot` it is the whole function.

What decides the guess, in order of confidence:

- **Inference** from the graph (decision 1). Free, sound, and enough for `loop`, `mandelbrot`,
  `nbody`, `collatz` and the numeric half of `fib`.
- **TypeScript annotations.** `function dist(this: Point): number`, `x: number`. Erased and
  unsound, so never trusted -- *guarded*, like any other guess, at the function's entry. This is
  the piece node cannot have: V8 rediscovers at runtime what the program's author wrote down. A
  guard that fails costs one boxed call; a guard that holds compiles the body unboxed.
- **A type profile.** `--profile native-profile` already instruments a build with gprof. It
  should additionally record, per site, the tags it observed, into a file the next build reads
  the way `optimize_ir(function, profile)` in design.md §2 always intended. This is PGO for
  types and it is the answer for untyped JavaScript: bundles, `node_modules`, the thirdparty
  corpus.

The cost is code size: a versioned loop is emitted twice. Versioning is therefore restricted to
regions that contain an arithmetic operation or a call on a value with a non-`unknown` type, and
a region is never versioned more than once. The generic clone is bit-for-bit today's output, so
`KILN_SPECULATE=0` -- a sibling of `KILN_KEYS_CHECK` -- produces today's binary and the
differential corpus can say whether the fast clone ever answers differently.

### 3. NaN-boxing, finished

ADR 0053 phase 3. Mechanical, large, and a prerequisite rather than a lever in its own right:
decision 2 produces values that live in `double` registers, and boxing one back must be a move,
not a two-word struct build. A number check must be one unsigned compare. A value must be one
argument register so that a typed entry (decision 4) with three parameters spills nothing.

The migration order is the one the ADR gives: `prelude_types.h` and the helpers exist; next the
emitted bodies (`locals`, `stack`, the call convention), then the prelude's hot paths -- the
object slots, the array elements, the call paths -- with `kiln_val_to_boundary` at the ABI. The
public `kiln_value` does not change (ADR 0021).

### 4. Calls become C calls

Three changes, in increasing ambition:

- **Direct calls to immutable bindings.** `kiln-ir` already computes `mutable_module_bindings`. A
  call to a module-scope `function` declaration, or a `const` bound to a function expression,
  that is never reassigned is a call to a *known function*: no closure load, no cell, no body-
  pointer comparison, a direct C call that clang can inline. Every recursive call in `fib` is one.
  Inside a closed program (below) the same holds across modules.
- **A typed entry beside the boxed one.** Every function keeps its `kiln_body_fn` entry, which
  is what a dynamic caller reaches. A function whose parameters and result acquire types from
  decision 2 also gets `static double fib_num(double n)`; the boxed entry guards, unboxes and
  calls it. A known-function call site with typed arguments calls the typed entry directly, so
  `fib` is real C recursion on doubles. This is where the 5x over node comes from.
- **Frame globals only when published.** `kiln_c99_invoke_closure_body` saves and restores
  nine globals per call. `publishes_frame` already says which functions need them; the boxed
  entry of a function that does not should be the body, not a wrapper around it.

### 5. Sites become inline guards

The inline cache exists (`kiln_c99_site`, four PIC entries keyed by shape) and lives out of line
in `kiln_c99_read_property` and `kiln_c99_op_call_method`. Emit the hit path inline: a shape
compare and a slot load in the C, and only the miss calls the prelude. On a hit at a call site,
the cached entry names the resolved method, so the call is direct (decision 4) and the callee's
`this.x` reads are slot reads under a guard the call site has already discharged.

Two things make that sound, and both are cheap:

- **A prototype epoch.** A cached prototype method is valid while no prototype on the chain has
  been written. An object that becomes some object's `[[Prototype]]` sets a flag; a write to a
  flagged object bumps one realm counter; the PIC entry records the counter it was filled under.
  V8 calls this a validity cell; one word does it here.
- **Intrinsics under a realm epoch.** `Math.sqrt`, `Math.round`, `Array.prototype.push`,
  `.length` on an array or a string: the emitter knows the receiver is the builtin (it emitted
  `LoadBuiltin(Math)`) and the method is the intrinsic unless the program has written to that
  builtin. The same counter, bumped by a write to any builtin object, guards `sqrt(a*a + b*b)`
  emitted as exactly that.

### The differentiator: a closed world

Everything above is what a JIT does, moved to compile time. The one thing a JIT cannot do is what
kiln should lean on hardest. `kiln build` links the whole module graph. If that graph contains no
direct `eval`, no `new Function`, and no dynamic import outside the graph -- and
`function_contains_direct_eval` already answers the first of those per function -- then the
program is **closed**, and whole-program facts hold without a guard:

- nothing writes `Math.sqrt`, so the intrinsic needs no epoch check;
- nothing assigns `Point.prototype.dist` after class definition, so the method is final and the
  call site's shape cache is the only dynamic test left;
- a class whose constructor writes `this.x`, `this.y` and nothing else, and whose instances are
  never written with a new key, has one shape, statically, and its property reads are struct
  loads.

An open program (dynamic code present -- ADR 0045 and 0051 make it possible) keeps the guards.
The guard is the fallback, not the design.

Two smaller AOT-only levers belong in the same section. A **regex literal** is a program: with
`regress` today every match interprets a PikeVM, and the review is right that Irregexp is a JIT.
An AOT compiler can lower the literal to a C matcher at build time, which is how the regex gap
closes without writing a JIT. And a numeric loop whose bounds and strides are static is one clang
can **vectorise**, which node's tiering never attempts; the hand-emitted `class-poly` runs
500,000 method calls in about half a millisecond because clang was allowed to.

## Order of work

Each step names the benchmark it moves and the number from the ceiling table it should reach.
The rule from `CLAUDE.md` holds throughout: every number is taken in a worktree, checksums must
match node's, and `mise run gate` is the acceptance test, because the generic path is what the
conformance corpora exercise and it must not move.

| step | what | moves | expected |
| --- | --- | --- | --- |
| 0 | definite-assignment TDZ elimination; drop diagnostics from paths that cannot refuse | every benchmark, a little | `loop` 286 -> ~200 ms |
| 1 | NaN-boxing phase 3: bodies landed in `cf93bb61`; the calling convention, prelude entry points and object slots remain | class B everywhere | `loop` -> ~120 ms, `fib` back below 92 ms |
| 2 | value graph; delete the fusers; guard hoisting; versioned numeric regions | `loop`, `mandelbrot`, `nbody`, `collatz` | `loop` -> ~27 ms, level with node |
| 3 | direct calls to immutable bindings; typed entries; frame globals only when published | `fib`, `frame`, every call | `fib` -> ~3 ms, ahead of node |
| 4 | inline site guards; prototype epoch; intrinsics; closed-world elision | `class-poly`, `collections`, `array-functional`, `object-spread` | `class-poly` -> ~0.5 ms, level |
| 5 | typed entries from TS annotations; type profile from `--profile native-profile` | typed and untyped real code | the thirdparty corpus |
| 6 | builtin sweep (string, JSON, buffer); regex literals lowered to C | class C | parity, each one measured |

Steps 0 and 1 are independent of each other and of everything after; step 1 is the large
mechanical one and should start now. Step 2 is the architectural one and depends on nothing but
step 1's representation. Steps 3 and 4 are each a few days on top of step 2 because the graph
gives them their facts; without step 2 each would be another family of fusers.

## What this does not claim

The hand-emitted C is a ceiling, not a forecast: a middle-end that reaches 80% of it on the three
programs above is a success, and the differentiated position -- 2.6 ms start, 8 MiB resident,
level with node on compute, ahead on typed code and on calls -- is reached well before 100%.

It does not close what the review says will not close. A getter, a proxy, a mutable prototype in
an open program, `arguments` in a sloppy function: each keeps its generic path and its guard, and
the last 20% there is not worth the machinery. What the architecture guarantees is that such a
program is never *slower* than today, because today's code is the generic clone.

## ADRs this touches

Extends 0052 and 0053; completes what 0050 called the next step and what 0047 called the last.
Keeps 0043 (the IR is unchanged), 0021 (one machine word internally, `kiln_value` at the
boundary) and 0041 (nothing here changes when collection happens). A value-graph middle-end and
compile-time deoptimisation each want an ADR of their own before the code lands; the numbers
above are the context section of both.

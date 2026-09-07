# Issue 04: ghui emits and compiles hundreds of megabytes of C

Status: in-progress

## Failure

The first successful ghui native-release build emitted 818,583,416 bytes of generated C and a
230,544,656-byte executable. Its cache contains 474 generated-C records; 112 are larger than one
megabyte and 13 are larger than ten megabytes.

The first fix separated the runtime implementation from its declarations. The materialized header
is 183,716 bytes, while the implementation source compiled once is about 2 MB. A precompiled-header
probe on the current largest translation unit made its O0 compile slightly slower (8.72 seconds to
9.02 seconds), so repeated header parsing is no longer the controlling cost.

The remaining cost is program-dependent generated statements. Even after compact emission, the
largest OpenTUI module contains 1,457 JavaScript bodies and produces an 18 MB translation unit. A
clang time trace attributes 4.53 seconds to parsing function definitions and 2.45 seconds to object
code generation for that unit alone. This is a representation problem, not a missing compiler
flag.

## Outcome

Decoupled module C includes one materialized runtime header by a stable basename. The native
pipeline makes the header digest an input to every module-object cache key and writes the header
once per build workspace. Runtime-provided modules obey the same emission policy.

Compact graph emission now also omits speculative clones, direct-call expansions, numeric shadow
paths, receiver-cache tables, compiled regular-expression clones, and repeated object/array/phi
statements. Inline-cache sites are grouped into arrays, module emission is bounded and parallel,
and release application glue is O0 while the shared runtime remains O3. The current cold ghui
result is 150,144,132 bytes of C; emission takes 1.54 seconds and compilation takes 15.59 seconds,
down from a 222-second O3 baseline.

The remaining outcome is a few-second cold compile. A real-codegen experiment split the largest
15.4 MB translation unit into five function-body shards: parallel clang time fell from 7.17 to
2.24 seconds. Whole-graph integration rejected that direction, however. Sharding every large body
raised generated C to 179 MB and left compilation at 15.22 seconds; limiting it to the three units
above 12 MB still raised generated C to 166 MB and compilation to 16.22 seconds. The graph already
fills all 16 CPUs, so reducing one straggler does not reduce its roughly 185 CPU-seconds of parsing.

A correctly guarded PCH saves about 30 ms on an otherwise-empty unit and nothing on a large unit.
The next reduction must therefore remove syntax and AST work across the entire graph: compact
operation tables or a similarly dense lowering, not more translation units or header machinery.

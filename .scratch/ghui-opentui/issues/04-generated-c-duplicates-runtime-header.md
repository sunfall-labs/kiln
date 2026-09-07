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
and release application glue is O0 while the shared runtime remains O3. A missed initializer
spelling had left compact-mode property and method sites as thousands of independent C statics;
grouping those too reduced the current cold ghui result to 145,298,364 bytes of C, 157,110,824
bytes of application objects, and a 106,148,232-byte executable. Emission takes 1.64 seconds and
compilation takes 15.95 seconds, down from a 222-second O3 baseline.

The declarations header also contained the complete method-group classifier even though generated
modules never call it. Keeping that implementation only in the shared prelude reduced the header
from 185,495 to 155,069 bytes, about 14 MB of repeated input over ghui's 474 modules. Adjacent cold
builds showed no compile-time win from that reduction: 16.89 seconds before compact-site grouping
and 15.95 seconds after it are still in the same band as the earlier 15.59-second result. The
cleanup is valid, but it confirms that parsing generated function bodies controls the build.

The remaining outcome is a few-second cold compile. A real-codegen experiment split the largest
15.4 MB translation unit into five function-body shards: parallel clang time fell from 7.17 to
2.24 seconds. Whole-graph integration rejected that direction, however. Sharding every large body
raised generated C to 179 MB and left compilation at 15.22 seconds; limiting it to the three units
above 12 MB still raised generated C to 166 MB and compilation to 16.22 seconds. The graph already
fills all 16 CPUs, so reducing one straggler does not reduce its roughly 185 CPU-seconds of parsing.

A correctly guarded PCH saves about 30 ms on an otherwise-empty unit and nothing on a large unit.
The next reduction must therefore remove syntax and AST work across the entire graph: compact
operation tables or a similarly dense lowering, not more translation units or header machinery.

A function-level status slow path was also tested and rejected. It reduced the largest unit's
isolated syntax check from 3.65 to 3.58 seconds and made the whole application object about 12 MB
smaller, but added 1.7 MB of generated C; adjacent full cold compiles remained in the same noisy
15--17 second band. Rearranging equivalent C control flow is not the required order-of-magnitude
change. The graph itself still contains both React reconciler variants (11.4 MB development and
6.4 MB production) and pulls 10.5 MB of `fast-check` through Effect's `sideEffects: false` graph.
Removing unused modules and declarations before C emission is the next high-leverage boundary.

The first whole-graph liveness pass now honors only the unambiguous package-wide forms
`"sideEffects": false` and `"sideEffects": []`. Named imports are resolved straight through pure
barrels to their canonical origins; unread imports and pure side-effect-only edges do not reach C.
Packages without that explicit permission, CommonJS graphs, namespace contents, and non-empty
side-effect pattern lists remain conservative. On an isolated cold ghui build this reduced C from
145,298,364 to 110,929,436 bytes, application objects from 157,110,824 to 119,823,328 bytes, and
compiler time from 15.95 to 12.19 seconds. The whole no-cache, one-shot command took 19.12 seconds
wall (1.87 seconds emit and 1.35 seconds link). `--explain` adds about forty seconds on this graph
and must not be included in an ordinary build-time comparison.

That is a useful 24% C reduction and 19% compile reduction, not the required endpoint. The seven
remaining units above 5 MB account for about 72 MB by themselves: three OpenTUI bundles, both
React reconciler branches, fast-check, and Effect Schema. Getting to a few seconds requires
export/declaration liveness inside those modules (and release-aware selection of React's CommonJS
branch), not more work on the 156 KB declarations header.

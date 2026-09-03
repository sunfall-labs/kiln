# ADR 0051: In-process dynamic evaluation in the native realm without external engines

## Status

Accepted. Amends ADR 0045 and fulfills the single-realm dynamic execution requirements of ADR 0036.

## Context

JavaScript programs frequently generate and execute code at runtime. While the native backend
statically compiles ahead-of-time (AOT) module graphs into C99 translation units, runtime code
generation via `new Function(args, body)` and `eval(source)` is fundamental to modern libraries:

1. Schema compilers (e.g., `ajv`) construct optimized validator functions on the fly.
2. Route matchers (e.g., `find-my-way` in Fastify) compile regex and trie dispatchers.
3. Template engines and serializer generators assemble specialized functions from dynamic schemas.

ADR 0030 noted that compiling strings into C at runtime would require invoking an external host C
compiler (`cc`) on temporary files, introducing severe latency, disk dependencies, and dynamic
linking fragility. ADR 0045 subsequently converted uncatchable process refusals into catchable
`EvalError` throws so that libraries like `zod` with explicit fallbacks could run.

However, libraries like `ajv` have no interpreted fallback: they require `new Function` to
produce callable functions. The tempting alternatives each violated core repository architectural
commitments:
- **Embedding a foreign JS engine** (e.g. QuickJS or V8) violates the single-realm invariant
  (ADR 0036), duplicates memory management with two separate garbage collectors, introduces
  heavy FFI marshalling overhead between incompatible value representations, and inflates
  binary size by 2 MB to 30 MB.
- **Ambient compiler subprocesses** require writable temporary directories, dynamic linker
  re-entry, and an external C toolchain at deployment time.

## Decision

**The native C99 runtime embeds a zero-dependency, in-process recursive-descent dynamic evaluator
that operates directly on the native realm's value kernel (`kiln_value`).**

### Key Mechanisms

1. **Direct Value-Kernel Execution**:
   The dynamic evaluator operates entirely on `kiln_value` tagged union values. It has no
   private value model, no secondary heap, and no FFI boundary. All operations—arithmetic,
   relational comparisons, bitwise logic, and equality—delegate directly to the native backend's
   existing operator kernels (`kiln_c99_binary` and `kiln_c99_unary`).

2. **Unified Closure Dispatch (`kiln_c99_dynamic_body_trampoline`)**:
   Functions constructed via `new Function` or dynamically compiled closures are standard
   `KILN_VALUE_FUNCTION` objects created by `kiln_c99_make_closure`. When called by native AOT code,
   they enter through `kiln_c99_dynamic_body_trampoline` and dispatch using standard Kiln calling
   conventions (`kiln_c99_call`).

3. **AST-Free, Zero-Copy Lexing & Parsing**:
   The evaluator uses a single-pass recursive-descent parser. String slices and identifiers are
   borrowed directly from the source buffer (`const char *`, `size_t len`) without intermediate
   AST node allocations.

4. **Lexical Scope & Control Flow**:
   Lexical scopes are managed with lightweight stack frames that record binding lengths for
   fast `memcmp` lookups. Outer-scope mutations use `kiln_dyn_scope_update` to preserve JavaScript
   closure semantics across nested blocks. Labeled loops (`outer0: for...`) and labeled breaks
   propagate through loop frames cleanly.

5. **Integrated Exception Handling**:
   Syntax errors and runtime exceptions populate `thrown` via `kiln_c99_throw_error` using
   standard builtin error identifiers (`SyntaxError`, `TypeError`, `ReferenceError`, `RangeError`),
   unwinding directly into native `try ... catch` exception blocks.

## Consequences

- **Ajv Validation Fully Unblocked**: `ajv` compiles schemas via `new Function` and executes
  natively with 100% fidelity to Node.js, passing all conformance checks.
- **Minimal Footprint**: The dynamic evaluator adds only **~40 KB** of compiled object code to
  the native runtime prelude (`runtime object`), with **zero external dependencies**.
- **No FFI Overhead**: Function calls and property reads between statically compiled C code and
  dynamically evaluated code incur zero marshalling or serialization latency.
- **Single-Realm Invariant Preserved**: Variables and functions share the same `globalThis`,
  intrinsic prototype tables, and garbage-collected heap.

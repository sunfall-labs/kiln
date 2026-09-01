# ADR 0036: Dynamic source compilation uses the realm value kernel

## Status

Accepted. This refines ADRs 0028 and 0030. It records the native architecture required before
`eval` or `Function` can be advertised on a native execution path; implementation is in progress.

## Context

ADR 0030 injects a `SourceCompiler` into the interpreter instead of making the VM depend on Oxc.
That is sufficient while source and execution both remain Rust values in one interpreter heap.
The native backend has a harder boundary: each generated C translation unit currently embeds its
own private value, object and call machinery. A Rust provider owns the realm scheduler and HostOps,
but it does not own JavaScript values.

Fastify makes the distinction observable. `find-my-way` builds optimized route matchers with
`new Function(...)`. Those functions close over constructor arguments and later read and mutate
ordinary application objects. Compiling the string in the interpreter would create a second heap:
the generated function could not preserve the identity of the C objects passed to it. Compiling a
shared library and returning one module's private `kiln_closure` to another module has the same
fault in a different direction: the object layouts look alike but their helper state, globals and
ownership are not one realm.

The tempting partial solutions are therefore wrong, not merely incomplete:

- interpreting only the generated string loses native object identity;
- recognizing Fastify's generated strings creates a package-specific language implementation;
- invoking a host compiler process ambiently makes execution depend on `cc`, writable temporary
  storage and dynamic-loader policy that the realm never named;
- precompiling strings found in source cannot cover strings assembled from runtime values;
- exposing Oxc or Tokio types through the generated-C ABI couples application objects to compiler
  and scheduler implementation details.

## Decision

### One realm owns one value kernel

The shipped Rust runtime accepted by ADR 0028 becomes the owner of JavaScript value allocation,
objects, property stores, callable metadata, queues and dynamic-code handles. Generated application
units retain separately compiled instruction bodies, but operate through a private versioned value
kernel ABI rather than embedding private copies of the heap/runtime implementation.

The public embedding `kiln_value` remains a boundary record. It is not the internal value
representation, as proposed by ADR 0021. Generated bodies carry opaque internal values supplied by
the realm kernel; no Rust layout, `Rc`, Tokio type, Oxc AST or allocator ownership crosses the ABI.

This migration is required independently of dynamic code. It also removes per-translation-unit
method registries, diagnostic fallbacks and other state that can only approximate a realm when
duplicated. The application object cache remains independent of the shipped runtime identity, as
ADR 0009 and ADR 0028 require.

### `SourceCompiler` is an explicit realm capability

A native realm has one of two source-code policies:

    source_compiler  unavailable (default) | injected(versioned compiler service)

The default is inert and deterministic. Reaching `eval` or `Function` without the capability is an
engine refusal, not a JavaScript `SyntaxError` and not an approximation. A Node-compatible realm
explicitly injects the service. The service uses the same Oxc frontend and Kiln IR compiler as the
ordinary build, but neither dependency enters the value kernel or generated module ABI.

The policy participates in artifact identity. The identity records absence or the compiler
service's semantic version/source identity; it does not record a transient process identity or
temporary path. A standalone that advertises dynamic source compilation must carry or explicitly
connect to that service. It may not discover a platform compiler from `PATH` at execution time.

### Compiled source returns an opaque code handle, not a foreign closure

The compiler service accepts owned UTF-8 source, parse goal, strictness and the compiler-described
scope contract. It returns an opaque, realm-owned code handle plus immutable callable metadata.
The value kernel creates the ordinary function object and owns its properties and prototype. Calls
enter the compiled body with kernel-owned receiver, arguments, cells and thrown-value slots.

The cache key covers normalized source bytes, parse goal, strictness, visible-scope descriptor,
compiler identity, IR version, private value-kernel ABI, target and profile. Syntax failures and
unsupported lowering remain distinct. Handles are released by realm teardown even if JavaScript
never observes a finalizer.

`Function` and indirect `eval` compile in global scope. Direct eval continues to use ADR 0030's
compiled `EvalSite` and shared binding cells. Native `Function` lands first because it requires no
caller-local scope, but it uses the same service and handle contract rather than a second API.

### Dynamic loading is a runtime implementation detail

An implementation may execute a code handle through a linked bytecode engine, a target-code cache,
or a dynamically loaded application body. That choice is below the handle contract. If native
machine code is loaded, the runtime package owns executable-memory and loader policy, validates the
private ABI and content digest before entry, and never hands a platform library handle to generated
code.

Tokio remains the sole runtime owner. Compilation is CPU/blocking work submitted through its
blocking facilities or completed before realm entry; JavaScript continuations resume only on the
realm's local task. Compiler completion cannot call into the heap concurrently.

## Migration sequence

The first migration slice is complete: runtime-provided callable property stores now use an
allocation-free lookup-or-publish ABI. The realm owns the winning opaque store and its destructor,
separately emitted modules observe one identity, and realm teardown releases it exactly once. This
is deliberately a bridge into the kernel, not the kernel extraction itself; generated closures,
ordinary objects and other per-unit state still follow the remaining sequence below.

1. Extract the internal value/kernel declarations from emitted module source and give the shipped
   runtime an explicit private export list.
2. Route allocation, property access, calls, diagnostics and realm queues through that kernel while
   retaining generated C instruction bodies and existing application-object cache keys.
3. Delete the per-unit stores and fallback runtime state once differential tests prove every
   advertised path uses the shared kernel.
4. Add the versioned `SourceCompiler` capability and content-addressed code-handle cache.
5. Enable `Function`/`new Function`, then indirect eval, then direct eval with ADR 0030 scope cells.
6. Gate Node compatibility on the same source-compiler policy in interpreted, native executable and
   embedding paths; run the pinned Node/Deno-derived tests and the Fastify production graph on each
   path that advertises it.

## Consequences

- Fastify, Ajv and other runtime-code-generating server packages can be supported without a
  package-specific evaluator or a second JavaScript heap.
- Deterministic realms retain an auditable refusal unless dynamic compilation is explicitly part of
  their inputs.
- The shared value kernel is a larger migration than adding one builtin, but it deletes duplicated
  runtime state and completes rather than circumvents ADR 0028.
- A native Node-compatible standalone grows by the compiler-service payload it selects. Programs
  that do not select dynamic compilation do not pay that cost.
- Oxc remains the parser/frontend at an injected boundary. It is relied on for syntax and semantic
  analysis, not asked to provide a runtime, heap or Node compatibility layer it does not contain.

## Alternatives rejected

**Run dynamic source in the interpreter beside native code.** Two heaps cannot preserve object,
closure, symbol, promise or error identity. A scalar bridge would pass toy arithmetic and fail real
generated server functions.

**Shell out to Kiln or a C compiler.** This adds ambient executable, filesystem and loader
dependencies to program execution and makes a standalone cease to be standalone.

**Special-case generated matcher templates.** This makes third-party source text part of Kiln's
runtime ABI and moves every dependency upgrade into the engine.

**Adopt a second JavaScript engine for eval.** It brings a second heap, event loop and language
implementation, contradicting the reason Kiln has its own compiler and ADR 0028's single-runtime
decision.

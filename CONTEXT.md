# Kiln domain context

Kiln compiles JavaScript and TypeScript through one executable semantic boundary into two
execution forms. This glossary names the architectural concepts used by code, ADRs and plans.

## Compilation

**Owned HIR** — Parser-independent source representation in `kiln-hir`. Oxc syntax types do not
cross this boundary.

**Bytecode** — Versioned, cached executable semantics produced by `kiln-ir`. The interpreter runs
it directly and the native backend abstract-interprets it into the Value Graph. Bytecode, not HIR,
is the common coordinate system for execution order, diagnostics and reconstruction (ADR 0061).

**Value Graph** — Backend-neutral SSA control/data-flow representation constructed from bytecode
by `kiln-backend-c`. It makes values, effects, exceptional edges and suspension edges explicit for
verification, optimization and native lowering. Do not call it the bytecode IR or a second source
lowering.

**Graph Verifier** — The authority that rejects an invalid Value Graph: dominance, control-flow,
types, handlers and reconstruction maps are compiler invariants rather than emitter assumptions.

**Native backend** — The path that lowers the Value Graph to C99 and invokes a host C toolchain.
Generated C is a backend artifact; it is not the language-semantics authority.

**Interpreter** — The deterministic bytecode VM in `kiln-interpreter`. It is the immediate
edit-run path and the reference execution used in native differential checks; it is not permitted
to define a second version of shared semantic algorithms.

## Execution

**Realm** — The owner of JavaScript global state, intrinsic identities, ordering queues and the
single mutator thread. Tokio schedules readiness; the realm decides observable JavaScript order.

**Turn** — One realm callback plus its specified queue processing. The native collector runs at
turn boundaries, where mutator state is quiescent.

**Execution capability** — An explicit source of environment-dependent behaviour such as clock,
entropy, timezone, locale or filesystem authority. A capability is supplied to an execution; it is
not read ambiently by language semantics.

**Host operation** — A service supplied by an embedder, such as I/O. Host operations cross
`kiln-host-abi`; regular expressions, numeric conversion and other language meaning do not.

## Semantic ownership

**Authoritative semantic owner** — The one implementation or generated description that decides an
observable operation (ADR 0060). Both engines adapt their local representation to it.

**Shared semantic engine** — A nontrivial language algorithm implemented once in Rust and reached
directly by the interpreter and through the native provider by emitted code. JSON, regular
expressions, BigInt and Temporal are examples.

**Representation operation** — Engine-local heap mechanics whose implementation must differ:
allocation, slots, shapes, rooting, write barriers and result materialisation. Its observable
contract remains shared even though its storage is not.

**Temporary duplicate** — Two semantic implementations retained during a migration with a named
differential authority and deletion condition. It is debt with an exit, not an ownership model.

**Refusal** — A source-attributed stop for unsupported semantics. Refusal is a deliberate answer;
absence or silent `undefined` is not. It is also a debt: kiln is a runtime, so every refusal is a
member the interpreter answers and the native backend has not yet, with a place in the backlog
and a baseline that only goes down (ADR 0063).

**Native provider** — The private, statically linked Rust archive exposed through the registry in
`kiln-native-number-abi`, packaged through `kiln-native-provider-contract`, and implemented by
`kiln-native-number-ffi`. It owns coarse realm,
semantic-engine, concurrency and foreign-interface operations used by emitted C. Do not call it the
number provider; numbers are one service it contains.

**Provider ABI registry** — The ordered, versioned list of native-provider exports, signatures,
feature groups and semantic owners. It generates or verifies Rust signatures, C declarations,
manifest exports and packaged symbols.

**Intrinsic description** — The future typed row from which a realm constructs an ordinary
built-in object and both engines derive surface and binding adapters (ADR 0059). The present
`kiln-builtin-surface` table describes only part of that contract and is a migration input.

## Testing and artifacts

**Native parity** — Identical observable output and exit status between the interpreter and native
backend for one bytecode program. Parity validates adapters and lowering; it is not a substitute
for one semantic owner.

**Corpus** — A declared collection of programs run by its repository-specified runner. Test262,
native, differential, third-party, Node, server, guard, effect and regress corpora answer different
questions and are not interchangeable.

**Provider capability** — One independently demandable engine supplied through the native-provider
ABI: Realm, RegExp, BigInt, JSON, Temporal, Intl, parallel execution, HTTP or FFI. Liveness computes
the required set; a manifest declares the supplied set; packaging chooses neither.

**Provider manifest** — The shipped description of one provider archive: schema, ABI, target,
variant, canonical capability set, source and archive digests, compiler identity and ordered
exports. Its identity is part of native artifact caching and installed-provider discovery.

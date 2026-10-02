# 16. The closed-world build behaviours of ADR 0067

Status: ready-for-agent
Blocked by: 04

Implement what ADR 0067 decides for `kiln build`, in the order the corpus needs it:

1. **Generated code (§1).** Precompile `new Function` / `eval` when the source is determined at
   build time, starting with ajv's and fast-json-stringify's output for Fastify's schemas. The
   ADR 0051 evaluator stays the fallback, so this is performance, not correctness.
2. **Computed `require` (§2).** Resolve provably finite sets; read a declared include list from
   `package.json` and the command line; refuse the rest naming the call site.
3. **Optional dependencies (§3).** A missing optional dependency throws `MODULE_NOT_FOUND` at run
   time exactly as Node does, with no build warning.
4. **Files read at run time (§6).** Embed provable paths and declared assets and serve reads from
   the binary; warn at build for any other path.

Choose the name and shape of the declaration key in `package.json` once, here, and document it in
the compatibility page.

## Acceptance

- Fastify's startup validators run native in the Fastify corpus app, and its throughput is recorded
  before and after.
- A probe for each of the four behaviours in `conformance/apps` or `conformance/node`.

## Comments

### 2026-09-27, integrator

`worker_threads` landed (realms issue 06, `512f769b`) and discovers `new Worker(...)` targets at
build time, including through `const` bindings, `||`/`??` and `path.resolve` calls. pino transports
and piscina still fail their deploy acceptance for exactly this issue's reason: their worker scripts
load the user's module from a path known only at run time (pino's `target`, piscina's `filename`).
The declared-include list from ADR 0067 section 2 is the fix; implement it with these two as the
first users.

### 2026-09-27 -- declared module roots for worker targets

The `work/worker-deploy` branch implements the `package.json` key `kiln.include`, with relative
entries resolved from the package root. The CLI sends canonical declared targets and their source
closure to the daemon in its pinned resolution snapshot; the graph links them as lazy roots and
the interpreter and native loaders can find their absolute path or file URL at run time. The
snapshot identity includes the declared list. The build must still refuse a missing declaration
by name. The pinned `conformance/apps/worker-deploy/` fixture proves pino transport in the kiln
interpreter; native pino and both piscina modes still have Node API blockers recorded under realms
issue 06. The general deploy app runner remains issue 02.

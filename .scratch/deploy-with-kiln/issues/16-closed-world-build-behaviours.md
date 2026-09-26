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

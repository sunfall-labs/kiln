# 06. Closed-world policy: dynamic code, addons, module patching

Status: ready-for-human

A native build is closed-world; a Node app sometimes is not. Decide, in an ADR, what `kiln build`
does with each of these, because each one is a different answer and users will hit all of them:

| shape | example | options |
| --- | --- | --- |
| `new Function` / `eval` of generated code | ajv and fast-json-stringify (Fastify), template engines | ADR 0051's in-process evaluator, or precompiling at build time when the source is a build-time constant |
| `require` with a computed specifier | plugin loaders, `require(path.join(dir, name))` | resolve when it is provably one of a finite set, include a declared list, or refuse naming the call site |
| native addons (`.node`) | `bcrypt`, `better-sqlite3`, `sharp` | refuse at build naming the package and suggesting the pure-JS alternative, or load through `node:ffi` |
| optional dependencies | `pg-native`, `utf-8-validate`, `bufferutil` | treat a missing optional dependency exactly as Node does when it is not installed |
| module patching | OpenTelemetry's `require-in-the-middle`, `import-in-the-middle` | a build-time instrumentation hook, or refuse |
| files read at run time | templates, `.wasm`, locale data | embed on declaration, or refuse naming the path |

The principle from ADR 0063 applies: each refusal is a debt with a place in the backlog, not an
end state. The ADR records which ones are debts and which are permanent, with the reason.

## Acceptance

- An accepted ADR covering every row, numbered with `ls docs/adr/` checked first.
- Issue 05's refusal message for each shape follows the ADR's wording.

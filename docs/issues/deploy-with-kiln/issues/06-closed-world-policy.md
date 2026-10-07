# 06. Closed-world policy: dynamic code, addons, module patching

Status: resolved

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

## Resolution, 2026-09-26

Decided by the project owner and recorded as ADR 0067:

| shape | decision |
| --- | --- |
| generated code | precompile natively when the source is known at build time; otherwise ADR 0051's evaluator |
| computed `require` | include a provably finite set; accept a declared list; otherwise refuse naming the call site |
| optional dependencies | behave as Node does: `MODULE_NOT_FOUND` at run time, no build refusal |
| native addons | load through N-API over `node:ffi`: the subset `bcrypt`, `better-sqlite3` and `sharp` use, after M2; refuse naming the package and an alternative until then |
| module patching | refuse now, as a debt; a build-time instrumentation hook in a later ADR |
| files read at run time | embed provable paths and declared assets; otherwise read from disk beside the binary, with a build warning naming the path |

Follow-on work: issue 16 (the build behaviours), issue 17 (N-API), issue 18 (the instrumentation
hook).

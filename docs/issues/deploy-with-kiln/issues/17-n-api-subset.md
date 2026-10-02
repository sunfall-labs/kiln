# 17. N-API: the subset real addons use

Status: ready-for-agent
Blocked by: M2 (issues 07, 08, 09)

ADR 0067 §4: `.node` addons load at run time through an N-API implementation over kiln's values,
built on `node:ffi`. `node:ffi` today is `dlopen`-style foreign function calls; nothing implements
N-API yet.

1. Measure: list the `napi_*` symbols `bcrypt`, `better-sqlite3` and `sharp` import (from their
   prebuilt binaries' dynamic symbol tables). That set, not the whole of N-API, is the first scope.
2. Implement it for both engines. The value handles, references, scopes and finalizers are
   representation operations (ADR 0060 kind 4); everything else is written once.
3. Addons ship beside the binary (ADR 0067 §4). Decide here whether to embed and extract instead.
4. Until an addon's symbols are all implemented, `kiln build` refuses it naming the missing
   `napi_*` functions and an alternative (`bcryptjs`, `node:sqlite`).

## Acceptance

- Corpus apps using `bcrypt`, `better-sqlite3` and `sharp` match Node natively.
- A list, in this file, of which `napi_*` functions are implemented and which addon needed each.

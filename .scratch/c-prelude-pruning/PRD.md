# PRD: Prune Superseded C Prelude Built-in Implementations

Status: active
Feature: c-prelude-pruning

## Background & Problem Statement

Per ADR 0005 and `AGENTS.md` ("Freeze `prelude_core.c` (Phase 5.1)"), built-in implementations are frozen in C and migrated to TypeScript.
In Phase 5.2, all 27 Node built-in modules (`node:*`) were migrated to pure TypeScript in `crates/kiln-builtins/src/node/*.ts`.
When programs run in native mode, `kiln-module-graph` resolves and compiles these TypeScript sources into native C compilation units.

However, the legacy C implementations of several of these built-ins were left behind in `crates/kiln-backend-c/src/runtime/`:
- `foundation.c`: `kiln_c99_node_path_answer` and `kiln_c99_node_path` (`isAbsolute`, `dirname`, `basename`, `extname`, `join`, `resolve`, `relative`).
- `web.c`: `kiln_c99_node_querystring_parse` (form decoding, dictionary mode allocation, property insertion).
- `node_support.c`: `kiln_c99_node_dns_lookup`, `kiln_c99_node_channel`/`diagnostics`, and `kiln_c99_node_util` (`debuglog`, `format`, `inspect`, `inherits`).
- `dispatch.c`: Obsolete dispatch cases calling the above functions, plus obsolete stubs (`NODE_HTTPS_CREATE_SERVER`, `NODE_HTTP2_CREATE_*`, `NODE_ASYNC_LOCAL_STORAGE`).

These dead C implementations add hundreds of lines to the C prelude header, increase C compiler compile times, and add maintenance friction.

## Goals

1. Remove dead C implementations of `node:path`, `node:querystring`, `node:dns`, `node:diagnostics_channel`, and `node:util` from `crates/kiln-backend-c/src/runtime/`.
2. Remove their obsolete call sites in `dispatch.c`.
3. Update `ABSENT_ANSWER_SITES` in `prelude.rs` (removing `kiln_c99_node_diagnostics` and `kiln_c99_node_util`).
4. Update `prelude_declarations.h` and method group mappings in `kiln-liveness` and `facts.rs`.
5. Update tests in `crates/kiln-backend-c/src/tests.rs` and update snapshots.
6. Verify 100% test pass on `mbx test -p kiln-backend-c` and `./scripts/native.sh`.

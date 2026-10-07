# PRD: Ambient TypeScript Runtime Bootstrap & Native Intrinsics

Status: completed
Branch: `feat/ambient-ts-prelude`

## Problem Statement

ADR 0005 and `AGENTS.md` ("Freeze `prelude_core.c` (Phase 5.1)") require new built-ins to be written in TypeScript with intrinsics rather than expanding the C prelude (`crates/native/kiln-backend-c/src/runtime/`). Adding built-ins to C is an explicit review refusal.

While Phase 5.2 successfully migrated Node built-in modules (`node:*`) to pure TypeScript in `crates/semantics/kiln-builtins/src/node/*.ts`, ECMAScript global built-ins and prototype methods (`Uint8Array.prototype`, `JSON`) have lacked an ambient TypeScript runtime prelude. As a result, recent changes had begun writing complex JavaScript semantics (options parsing, revivers/replacers) directly in C, causing cache invalidation and maintenance debt.

## Architecture

1. **Ambient TypeScript Runtime Bootstrap**:
   - Leverage and expand `@kiln/runtime-bootstrap.ts` (`NATIVE_BUILTIN_BOOTSTRAP_SOURCE`), which executes at realm setup before user code.
   - Implement ECMAScript prototype methods (`Uint8Array.prototype.toHex`, `Uint8Array.prototype.toBase64`) in TypeScript.
   - Implement `JSON.parse` with revivers and `JSON.stringify` with replacers in TypeScript wrapping the native JSON primitives.

2. **Minimal Native Intrinsics**:
   - The C runtime exports low-level raw intrinsics (`__kilnUint8ToHex`, `__kilnUint8ToBase64`) to `globalThis`, which are captured and deleted by the bootstrap module during realm initialization.
   - Intrinsics perform only raw memory/byte operations with zero JavaScript options parsing or error formatting in C.

3. **Prototype Store Delegation on Typed Arrays**:
   - In `read.c` and `enumeration.c`, allow `KILN_VALUE_TYPED_ARRAY` instances to consult the prototype's `callable_store` when a member is not in the static method table, ensuring methods installed by the bootstrap are callable on all instances.

## Goals

- 100% semantic parity between `kiln-interpreter` and `kiln-native` on `conformance/native/typed-array-codecs-refuse.ts` and `conformance/native/json-roundtrip-edge-refuses.js`.
- Strict adherence to ADR 0005 and Phase 5.1 freeze on `prelude_core.c`.
- Zero compiler/runtime regressions in `scripts/native.sh`.

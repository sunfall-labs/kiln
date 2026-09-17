# Run Pi in Kiln

Status: ready-for-agent

## Why

[Pi](https://github.com/earendil-works/pi) (`@earendil-works/pi-coding-agent`, `@earendil-works/pi-agent-core`, `@earendil-works/chord`) is a modern AI coding agent built with clean TypeScript architecture.
Unlike other agents that rely on complex meta-frameworks, Pi's core runtime is modular and self-contained. Bringing full compatibility for Pi into Kiln makes Kiln a first-class compiler and runtime for state-of-the-art agent frameworks.

Furthermore, running Pi exposes key compiler capabilities that Kiln should natively provide without requiring an external bundler:
1. Complete TypeScript support (ambient function overloads, namespaces, enums, parameter properties).
2. Monorepo workspace root resolution for multi-package projects.
3. Node compatibility extensions (`node:process`, `child_process`, `net.isIP`, `fs.chmodSync`, `timers/promises`).

## Verified Baseline

- Target project: `../oss/pi` at commit `e4c75a73222ae2c72abb5f5314fa35ee8effc508` (v0.85.1).
- Bundled packages (`@earendil-works/chord` and `@earendil-works/pi-agent-core`) already pass 100% in Kiln's interpreter and achieve 100% native execution parity when pre-bundled.
- Live `Agent` state machine, system prompts, and streaming callbacks execute natively with Clang.

## Incremental Plan

### Milestone 1: Native TypeScript Lowering without Bundler
- [ ] Fix ambient function overload declarations (`declaration.body.is_none()`) so they are discarded as type-only declarations rather than failing with `unsupported statement: function declaration`.
- [ ] Enable `oxc_transformer` for TypeScript sources (`source_type.is_typescript()`) so that namespaces, enums, and parameter properties are lowered directly into standard JavaScript AST for HIR lowering.
- [ ] Verify `kiln check` directly on `../oss/pi/packages/chord/src/api.ts` and `../oss/pi/packages/agent/src/index.ts`.

### Milestone 2: Workspace & Monorepo Root Resolution
- [ ] Update project root discovery (`find_project_root`) to recognize monorepo workspace roots containing `packages/` or git roots, avoiding `resolved module escapes project root` for internal cross-package dependencies.
- [ ] Verify direct resolution across Pi workspace packages.

### Milestone 3: Node Compatibility Surface Expansion
- [ ] Map `"node:process"` and `"process"` specifiers in `NodeBuiltin`.
- [ ] Add missing exports to existing builtins:
  - `node:net`: `isIP`.
  - `node:fs`: `chmodSync`.
  - `node:timers/promises`: `setTimeout`.
- [ ] Implement `child_process` adapter backed by Kiln's Tokio process runner.

### Milestone 4: Pi Integration & Acceptance
- [ ] Run Pi agent tests directly in Kiln without pre-bundling.
- [ ] Verify interpreter and native backend parity.
- [ ] Run `mise run gate-quick` and workspace tests to ensure zero regressions across all existing suites.

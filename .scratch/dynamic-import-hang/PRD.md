# PRD: Dynamic `import()` Promise Settling and Host Loader Resolution

Status: ready-for-agent
Branch: `feat/dynamic-import-settling`
Owner: Antigravity, 2026-09-06

## Problem Statement

When dynamic `import()` is invoked under `kiln run`, any load failure (such as unsupported dynamic import or absent loader) currently raises an immediate uncatchable machine `ExecutionError::DynamicImportUnsupported`, terminating the VM before the Promise can settle.

As a result:
```js
import("./dep.mjs").catch(err => console.log("caught:", err.message));
```
Never executes the `.catch` handler, and user-space error handling cannot catch or handle the error.

Furthermore, dynamic `import()` requires resolving relative specifiers against the importing module's directory rather than ambient state.

## Specification Requirements

Per ECMA-262 §13.3.10.1 (`HostLoadImportedModule`):
- If the host cannot complete loading an imported module, the failure must be communicated back by rejecting the promise returned by `import()`.
- Machine panics / uncatchable VM errors must not be thrown for module load rejections.

## Phased Approach

- **Issue 01** (Resolved): Ensure all module loader errors and unsupported loader paths reject through the returned Promise with a `TypeError` / `SyntaxError` rather than aborting the VM via unhandled `ExecutionError`.
- **Issue 02**: Connect referrer-aware module resolution so that `import("./relative.mjs")` can resolve relative to the importing file.

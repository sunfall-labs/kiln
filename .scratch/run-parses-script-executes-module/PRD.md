# PRD: Align Parser Goal with Execution Goal in `kiln run`

Status: resolved
Branch: `fix/run-parser-module-goal`

## Problem Statement

`kiln run foo.js` parsed files as Scripts (based on file extension via Oxc) but executed them as ES Modules under strict semantics. This permitted early script errors (`with`, octal literals, undeclared variables) to pass parse time and fail unexpectedly at runtime.

## Resolution

Updated `parse` in `crates/frontend-oxc/src/lib.rs` to set `.with_module(true)` for non-`.cjs` files, ensuring parse-time early error checks match runtime module semantics.

# Live ES module namespaces through CommonJS require

Status: implemented; focused validation passed. Whole-milestone gate pending.

A CommonJS module returning `require('./live.mjs')` previously received a new ordinary object with copied export values. Mutating the ESM binding later left the CommonJS default object stale and lost namespace identity.

Commit `3bc44376` returns the canonical ModuleMap namespace for ESM require targets, including the runtime fallback. CommonJS named-export materialization refreshes namespace slots before taking its one-time snapshot. The default remains the original live namespace.

Two regressions cover mutation after static import and mutation before a later dynamic CommonJS import. Node 26 agrees with the latter: `true true 2 2`, followed by `2 3`; the CommonJS named export remains 2 while the default namespace advances to 3. The proxy declares export names before assigning module.exports so Node's static CommonJS export scanner recognizes them.

Validation in owned worktree `~/Work/kiln-metric-cjs-loader`, source commit `e562b043`: `mbx test -p kiln-module-eval --jobs=1` passed all 18 module-map tests, with no failures. This proves the focused loader contract; it does not establish full upstream Effect or native conformance.

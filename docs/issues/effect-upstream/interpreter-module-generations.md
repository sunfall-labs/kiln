# Interpreter module generations and CommonJS cache policy

`resetModules` creates a fresh interpreter module generation for source-module imports. Static graph records remain in generation zero; closures and queued dynamic imports keep the generation they captured. Host modules are linked to the canonical host record so `require` retains its Node cache identity, while each import generation gets its own namespace wrapper and uses the host record's export cells and cached namespace snapshot/failure.

## Vitest boundary

The reference behavior was checked with the pinned Node 26.8.1 / Vitest 5.0.3 installation, using both a relative `.cjs` file transformed as project source by Vite and a fake package under `node_modules`:

- The transformed relative `.cjs` module is part of Vitest's module graph. `vi.resetModules()` followed by `vi.importActual()` evaluates it again; the probe's evaluation counter reaches three across `require`, the pre-reset import, and the post-reset import.
- The external package uses Node's `require.cache`. `require()` and `vi.importActual()` share the same `module.exports` object and execute its body once. The dynamic-import namespace object from before reset differs from the namespace after reset, while repeated imports within one generation share the same namespace.

These are different loader policies. The interpreter's host CommonJS loader currently models the external Node cache policy. It does not model Vite's transform of a project-relative `.cjs` file as an ESM graph module. The upstream `resetModules`/`importActual` files in the pinned Effect selection use TypeScript targets (`ConfigProvider.ts`, RPC error source, and Schema/HMR sources); those tests do not establish transformed-relative-CJS parity. Do not claim that behavior is implemented until the graph carries source-versus-external loader identity and has an oracle-backed regression for each policy.

## Lifetime follow-up

Generation records are held by the realm's `ModuleMap` in a strong map keyed by `(realm, generation, canonical path)`. Thus repeated resets currently retain old record state even after no language object or job can reach it. This preserves old closures and pending imports, but it also retains unreachable module namespaces, export cells, failures and values indefinitely. No generation reclamation is implemented here.

A plain `Rc` generation token is not enough: generation records retain closures, and those closures can retain the token, forming a cycle. A safe reclamation design should represent generation state through the existing GC owner graph, keep only a weak epoch registry, and treat the current runner generation as a strong root. Closures, namespaces/live cells that can perform old-generation imports, and pending module jobs must mark their captured generation; an unmarked inactive generation can then release its record set as a unit. Reclamation must be tested with an unreachable old namespace (collectible), a retained old closure (still imports from its generation), and a pending old dynamic import (kept through settlement); avoid scanning all records on each reset.

## Current focused evidence

The combined reset tests verify fresh source cycles, old namespace retention, canonical host export-cell identity, generation-local namespace identity, and cached CommonJS getter values/failures. The direct Effect selections `Config`, `EffectKeepAlive`, `StackTraceLimit`, `rpc/RpcClient`, and `schema/HMR` are the current targeted upstream validation set. This evidence does not cover every Vite/CommonJS transformation policy or prove old-generation records are reclaimed.

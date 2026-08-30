# ADR 0027: Node packages use a separate resolution policy

## Status

Accepted.

## Context

ADR 0011 deliberately made module resolution exact: relative and absolute source paths are
canonicalized and checked, while bare package names are rejected. That kept the first graph small,
but it also means an ordinary `import { Effect } from "effect"` cannot enter the compiler.

Putting Node rules directly into the exact resolver would weaken a useful Interface. Package lookup
has different inputs and different evolution pressure: ancestor `node_modules` directories,
`package.json`, export maps, conditions, and eventually workspace or Plug'n'Play adapters. The
Module graph needs one canonical target; it should not acquire those rules itself.

Source authority is the other half of the decision. The daemon never reads JavaScript source from
disk after synchronization. Recursively walking `node_modules` would preserve that rule at the cost
of making every command proportional to the installed dependency tree. Package discovery must be
entry-rooted and must synchronize only the sources selected by the same policy as graph linking.

## Decision

### Resolution is a policy Interface with separate Adapters

`ResolutionPolicy` maps one `(specifier, canonical importer)` pair to a canonical source path.
`ModuleResolver` remains the exact relative/absolute Adapter from ADR 0011.
`NodePackageResolver` delegates explicit paths to it and handles bare package specifiers separately.
The Module graph owns traversal and linking and accepts either policy; it does not parse package
metadata.

`NodePackageResolver` delegates Node's resolution state machine to `oxc_resolver`, including
conditional and wildcard exports, package imports, self-references and `main` fallback. Kiln
retains only its supported-source and project-containment policy. It does not guess extensions or
index files.

### Synchronization follows the entry-rooted dependency closure

For source-consuming CLI commands, the source-discovery Adapter starts with the already-read entry
snapshot, parses only its runtime dependency closure, and resolves each edge with the Node policy.
Ordinary project sources reuse the bytes captured by the authoritative project walk. Newly reached
package sources are read once, added to the same staged Sync manifest, and sorted with the other
paths. `node_modules` remains excluded from the broad project walk.

An active package source remains a watch dependency because it is present in the source manifest,
even though an unrelated file under `node_modules` is not discoverable. Package-source changes use
the same compare-and-swap update path as project dependencies.

### Package decisions are part of source authority

The client records every package edge selected by `oxc_resolver`, the exact `package.json` path and
its content digest in the staged `ResolutionMap`. Commit publishes that map atomically with source
bytes and `SourceView`. Daemon graph construction uses `PinnedPackageResolver`: explicit paths keep
the exact filesystem policy, while package requests are pure `(importer, specifier)` lookups. The
daemon therefore cannot observe a different manifest generation from the client that produced the
view. Resolution-map identity includes the manifest digest.

Alternative install layouts such as Plug'n'Play and packages outside the canonical project root
still require explicit policy additions.

## Consequences

- Deleting the Node Adapter leaves the explicit filesystem resolver and Module graph intact.
- Package work has high Locality: manifest parsing and ancestor lookup stay behind one policy
  Interface instead of spreading through graph, daemon, and native consumers.
- Synchronization cost follows reachable package sources, not installed package count.
- The same graph result continues to feed interpreter and native execution.
- Rich Node rules are inherited from a maintained Rust implementation without allowing its types
  to cross Kiln's resolver Interface.

## Acceptance checks

- Explicit relative/absolute resolution retains all ADR 0011 behavior.
- Scoped and unscoped packages use the nearest eligible `node_modules` directory.
- Package roots, conditional exports and wildcard subpaths resolve; blocked exports remain failures.
- Traversal and symlink targets cannot escape the selected package or project root.
- A one-shot command synchronizes only the reachable package sources, executes a named import, and
  observes a package-source edit on the next authoritative Sync.
- Watch recognizes a synchronized package source without broadening project discovery to all of
  `node_modules`.
- The staged source view pins the package metadata/resolution map, and daemon graph construction
  cannot observe a different manifest generation from the client that produced the view.

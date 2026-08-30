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

The first Node Adapter is intentionally bounded. It supports nearest-ancestor `node_modules`,
scoped and unscoped names, exact `exports` keys with explicit `null` blocking, and `main` when no
`exports` field exists. It does not guess extensions or index files and does not yet interpret
conditional or wildcard exports. Targets must be regular supported source files, remain inside the
canonical project root, and remain inside the selected package directory after symlink resolution.

### Synchronization follows the entry-rooted dependency closure

For source-consuming CLI commands, the source-discovery Adapter starts with the already-read entry
snapshot, parses only its runtime dependency closure, and resolves each edge with the Node policy.
Ordinary project sources reuse the bytes captured by the authoritative project walk. Newly reached
package sources are read once, added to the same staged Sync manifest, and sorted with the other
paths. `node_modules` remains excluded from the broad project walk.

An active package source remains a watch dependency because it is present in the source manifest,
even though an unrelated file under `node_modules` is not discoverable. Body and explicit-import
edits keep using the compare-and-swap fast path. A package-manifest edit, or a source edit that
changes its set of bare requests, performs a new authoritative resolution before execution.

### Package metadata is pinned as part of source authority

The selected `package.json` files influence graph identity even though they are not JavaScript
sources. Protocol 17 makes every staged Sync carry a canonical resolution map: project root plus
sorted `(importer, specifier, target, package.json, manifest digest)` entries. The map is validated,
strongly identified with length-framed SHA-256, and published atomically with the source bytes and
`SourceView`. A stale competing Sync publishes neither its sources nor its map.

Graph construction consumes only the committed map for bare imports; it never reopens live package
metadata. Native cache topology consumes the resolved canonical targets produced from that map.
Two maps that select the same complete graph may therefore reuse the same native artifact, while a
target change necessarily changes graph topology and link identity. This is content-cache reuse,
not a second resolution decision.

Conditional exports, wildcard exports, package self-references, workspace protocols, Plug'n'Play,
and packages outside the canonical project root require later explicit policy additions; they are
not guessed by the first Adapter.

## Consequences

- Deleting the Node Adapter leaves the explicit filesystem resolver and Module graph intact.
- Package work has high Locality: manifest parsing and ancestor lookup stay behind one policy
  Interface instead of spreading through graph, daemon, and native consumers.
- Synchronization cost follows reachable package sources, not installed package count.
- The same graph result continues to feed interpreter and native execution.
- Package resolution has the same source-view authority boundary as JavaScript bytes; live
  manifest changes take effect only through another committed Sync.

## Acceptance checks

- Explicit relative/absolute resolution retains all ADR 0011 behavior.
- Scoped and unscoped packages use the nearest eligible `node_modules` directory.
- Exact package roots and subpaths resolve; absent and `null` exports remain distinct failures.
- Traversal and symlink targets cannot escape the selected package or project root.
- A one-shot command synchronizes only the reachable package sources, executes a named import, and
  observes a package-source edit on the next authoritative Sync.
- Watch recognizes a synchronized package source without broadening project discovery to all of
  `node_modules`.
- The staged source view pins the package metadata/resolution map, and daemon graph construction
  cannot observe a different manifest generation from the client that produced the view.

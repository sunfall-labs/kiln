# ADR 0024: Source synchronization is staged and authoritative

## Status

Accepted.

## Context

The daemon is a resident compiler, while its clients are commonly stateless editor or CLI
processes. A client needs to describe its complete source set, upload only bytes the daemon does
not already have, and then run a query against the exact set it synchronized. A one-message
replacement protocol cannot provide all three properties: sending a large snapshot in one request
keeps the connection occupied, and applying files as they arrive exposes a partially uploaded
project to concurrent requests.

The source set also has two distinct kinds of freshness. The authoritative source view answers
whether the set of paths and bytes is still the set a client synchronized. The database revision
used by native compilation answers whether a prepared artifact may still be published. They can
advance for different reasons and must not be conflated.

## Decision

### Synchronization has begin, stage, and commit phases

`Request::Sync { files }` is begin-only. The daemon snapshots the client's manifest and the
current `SourceView`, allocates a connection-local `SyncId`, and computes `Response::Needed` from
the manifest's strong `SourceDigest`s. Computing that answer does not mutate the authoritative
source set: omitted active paths are not removed, and no incoming bytes become visible.

The client sends one `Request::StageSource { sync_id, path, text }` for each needed path. The
daemon hashes the exact UTF-8 text and accepts it only when the digest matches the manifest. These
bytes live in pending state owned by that connection and synchronization; they are not added to
the engine's source map or visible to queries while staging.

`Request::CommitSync { sync_id }` is serialized with the engine under its source lock. The daemon
rejects the commit with `Response::StaleSourceView` when the begin snapshot's base view is no
longer current, and rejects an incomplete or invalid pending set rather than publishing a partial
project. Once both checks pass, it atomically applies the manifest: paths omitted by the client
are removed and staged paths are added or updated in one transaction. The resulting response is
`Response::Synced { source_view }`.

Every successful commit increments `SourceView`, including a byte-for-byte no-op. That increment
is intentional: a successful authority boundary must fence concurrent clients even when the
resulting bytes happen to compare equal. A failed commit leaves the current source view and
authoritative source set unchanged when validation fails or its base view is stale.

`MemoEngine` input mutations are non-panicking. The generic daemon test seam nevertheless accepts
other `QueryEngine` implementations, so an unexpected panic during their input mutation cannot be
rolled back through that interface. The daemon catches such a panic while the state lock is held
and advances `SourceView` before reporting an internal error. No observer can see the intermediate
state under the old token, and the next client must reconcile it with a complete Sync.

### Queries consume an explicit source view

`Check`, `Run`, `NativeBuild`, `NativeBuildAtRevision`, `NativeSharedBuild`, `NativeRun`, and
`Project` carry `source_view`. Each checks that view while holding the same engine lock used to
prepare its source-dependent work. A request with an old view returns
`Response::StaleSourceView { current }` before preparation, so it cannot prepare against one
source set and execute against another.

The native `expected_revision` on `NativeBuildAtRevision` remains a separate publication fence.
Source-view validation protects the source authority boundary; expected-revision validation still
protects publication of a prepared native artifact when the database changes during the build.

`SourceView` is not derived-content identity. After a request proves that its view is current, the
module-graph cache looks up a bounded set of snapshots by canonical root and entry, pinned
resolution-map identity, and graph format. Each snapshot captures the database revisions of the
source files that graph actually reached. A byte-identical sync or an edit outside that reachable
set therefore advances the authority fence without discarding the graph. A reachable edit cannot
hit the snapshot, and a different resolution map selects a different key. An uncertain input-
mutation panic still clears every snapshot because a generic `QueryEngine` cannot promise which
revisions it published before unwinding.

### Watch updates remain compare-and-swap

`Request::SetSource { path, text, source_view }` is the low-latency watch/editor path, not a
replacement for complete synchronization. It compares the supplied view under the engine lock,
updates one path only when that view is current, and returns `Response::SourceUpdated { source_view }`
with the newly incremented view. A stale watch update returns `StaleSourceView` and does not
silently overwrite a newer authoritative snapshot.

### Pending synchronization is connection-scoped

Pending manifests, base views, and staged bytes are owned by the connection. Disconnecting drops
them without changing the daemon's source set. A client receiving `StaleSourceView` restarts a
bounded complete synchronization from a fresh manifest; it does not retry an old staged commit
indefinitely or attempt to infer which paths changed concurrently.

## Consequences

- A source snapshot becomes visible only at commit, so every query sees either the old complete
  project or the new complete project, never a mixture of staged files.
- Synchronization traffic is proportional to changed or missing file contents, while the manifest
  remains cheap to send on every stateless command.
- The source view is a monotonic authority fence. Clients must retain the view returned by `Synced`
  or `SourceUpdated` and recover with a complete resync after a stale response.
- A no-op complete sync still invalidates the authority token. This costs a view increment but
  prevents two clients from treating the same observed view as an unexpired write lease.
- Authority-token churn and edits outside a graph's reachable source set do not invalidate that
  graph. Several recently used entries remain warm behind a bounded LRU interface.
- Disconnect cleanup is automatic because pending state is not shared with the engine or another
  connection.
- Native builds retain two independent correctness checks: source-view freshness before prepare
  and database-revision freshness before publication.

## Acceptance checks

- A begin request returns `Needed` with a `SyncId` and does not add, update, or remove an active
  source.
- A staged file whose SHA-256 digest differs from its manifest entry is rejected and never becomes
  visible.
- A commit missing any needed path, or whose base view is stale, publishes nothing.
- An unexpected custom-engine mutation panic invalidates the old source view before another
  request can observe or consume the partially changed internal state.
- A successful commit applies removals and updates atomically and returns a strictly newer view,
  including when the resulting source bytes are unchanged.
- A byte-identical sync and an unrelated source edit retain a graph snapshot; a reachable source
  edit or pinned-resolution change cannot hit it.
- A source-consuming request with an old view returns `StaleSourceView` before preparing a query.
- A current `SetSource` returns `SourceUpdated` with the next view; a stale one leaves the source
  unchanged.
- Disconnecting a client drops its pending sync, and a complete resync can recover from a stale
  view within a bounded retry policy.
- `NativeBuildAtRevision` still refuses publication when `expected_revision` is superseded during
  native compilation.

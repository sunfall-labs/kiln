# Issue 09: the interpreter exits 0 on an unsettled top-level await

Status: fixed 2026-09-19
Feature: full-runtime

```js
await new Promise(() => {});
```

Node prints `Warning: Detected unsettled top-level await` and exits **13**. The native
launcher reports `top-level await: unsettled` and exits non-zero. The interpreter exits 0 and
prints nothing, so a program whose promise never settles -- a forgotten `resolve`, a server
that closed before the await -- reads as success to a shell or CI.

The interpreter already knows: `run_node_event_loop_until_module` returns with
`module_outcome()` still `None` when the loop has nothing left to run. That is the moment to
report it the way node does, with the warning on stderr and status 13, from
`execute_module_in_runtime_mode_with_session` (`crates/interp/kiln-interpreter/src/lib.rs`, after
the event loop, where a rejected outcome is already turned into an uncaught error).

Acceptance: `kiln run` and `kiln run --native` agree on the status for the one-liner above;
a `conformance/native` probe pins it (native.sh compares exit statuses).

## What landed

Both engines print node's warning and exit 13: the interpreter from
`execute_module_in_runtime_mode_with_session` once the entry's loop is out of work with the
module unsettled, the native launcher from `kiln_cli_run_realm` (`graph->unsettled_entry`).
Found beside it: the interpreter stopped the entry's loop the moment its top-level await
settled, so a timer armed by the continuation never ran; it now keeps draining.
`conformance/native/timer-after-top-level-await.mjs` pins the timer; the exit status is pinned
by `native_unsettled_top_level_await_warns_and_exits_13` (kiln-native) and
`node_entry_with_an_unsettled_top_level_await_exits_13` (kiln-module-graph), because
`native.sh` counts an interpreter run that exits non-zero as a probe nobody can run.

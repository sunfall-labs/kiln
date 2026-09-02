# Archive: the client path onto the engine, and the checkpoint loop goes

Status: done

`crates/kiln-native-number-ffi/src/lib.rs` (the runtime archive generated C links against) still
runs `HttpRole::Client` connections through the bridge: `attach_http` starts a bridged
connection, `checkpoint_http` pumps every client connection after every turn, `apply_http_actions`
lowers transport actions to host operations, `route_completion` routes their completions, and
`queue_http_event` boxes each decoded event into a microtask.

- `attach_http` attaches both roles through `RealmRuntime::server_attach` /
  `client_attach` with one `NativeHttpDispatch` (the server dispatch, extended with the response
  kinds, `status`, `reason` and `reusable` → `flags`).
- `kiln_realm_http_client_start/write/end_active` call the client engine.
- `kiln_realm_http_response_*_active` and `kiln_realm_http_server_close_active` lose their
  "is the resource attached to the direct engine" branch; close works for either role.
- `drive_async` drops both `checkpoint_http` calls; `requests_in_flight` includes the client
  engine, which is what keeps the drive waiting on an outstanding response.
- Deleted: `checkpoint_http`, `apply_http_actions`, `queue_http_event`, `HttpEventPayload`,
  `run_http_event`, `drop_http_event`, `WorkRoots::HttpEvent`, `http_requests`, the HTTP branch
  of `route_completion`, every `_in_context` call.

The raw-close refusal in `kiln_realm_start_host_op` (an HTTP-owned resource cannot be closed
through `CloseResource`) stays; `registered_http_connection_rejects_raw_hostops_close` pins it.

## Comments

- Done in `9471372` ("Archive: attach HTTP clients through the engine and delete the
  checkpoint loop"). Net -761 lines in `crates/kiln-native-number-ffi/src/lib.rs`. All 13
  `kiln-native` node:http fixtures pass, including both close-contract tests.
- The archive's dispatch had been putting an error's message in the view's `reason` slot while
  the generated kind-9 handler reads `data`; it now fills both.

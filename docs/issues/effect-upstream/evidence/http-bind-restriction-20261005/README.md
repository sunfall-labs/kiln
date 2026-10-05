# Current sandbox loopback restriction

The ongoing frozen-snapshot interpreter survey recorded eight HttpClient failures as ServeError. Independent Python and Node loopback-listener controls both fail with EPERM in this session. This establishes that the environment cannot run the local HTTP listener those tests require. The Effect failures are consistent with that restriction; their original outcomes remain recorded as failures, with no skips or exclusions added.

These controls are environment evidence, not proof that the current kiln HTTP implementation passes the upstream tests. Final acceptance still requires executing all cases in a host that permits their listeners. Shell GitHub DNS and connector repository access are separately blocked; the current goal remains active because runtime corrections and performance diagnosis can still progress.

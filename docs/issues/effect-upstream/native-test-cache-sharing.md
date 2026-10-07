# Sharing native test build artifacts

The native backend's Rust integration tests can reuse compiled artifacts across
owned worktrees by setting `KILN_NATIVE_TEST_CACHE_ROOT` to the same absolute,
writable directory for each test invocation. The setting affects only the
shared artifact-cache layer used by native test fixtures. Without it, the
existing default remains `<worktree>/target/kiln-test-cache`; fixtures that
provide private cache layers continue to do so.

This cache stores content-addressed build artifacts, not test verdicts. Tests
still build or retrieve artifacts by their normal keys and execute their
programs on every run. Keep the directory on a filesystem suitable for
concurrent worktree access, and do not remove or replace its
`.compile-locks/v1/` files while builds may be active. Normal cache garbage
collection can reclaim eligible artifact records after users of the cache
have finished.

An invalid configured path is an explicit test-setup error: it must be
nonempty and absolute, and directory-creation errors are reported with the
configured path. The unset environment path retains the previous default and
fallback behavior.

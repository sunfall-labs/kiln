# Pinned Node test-suite slice

This directory is the vendored suite boundary used by `node-compat.json`. Files retain their
Node.js test-suite paths and are pinned to Node v24.7.0 commit
`d795edb56765d4ec743bcda5a0c1452bed74de0c`.

Passing upstream tests may currently have harness-only placeholders here while the direct
`test/common` runner is built; executable adapted assertions remain in `../upstream-adapted` and
are the CI gate. Expected-failure entries are inventoried now so newly passing behavior is visible
and promoted deliberately rather than discovered through an undocumented ad-hoc test.

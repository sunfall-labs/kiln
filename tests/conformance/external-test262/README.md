# External-layout Test262 fixture

This tiny corpus mirrors the layout of an upstream Test262 checkout: `harness/` is beside
`test/`, and tests carry Test262 frontmatter. It is intentionally repository-owned so unit tests
can verify harness discovery and the resident batch without cloning the network suite.

The fixture covers harness-backed sloppy/strict scripts, an explicit module goal, a parse-negative
expectation, and a language feature that the current interpreter reports as unsupported. The
upstream lane uses the same `kiln test <checkout>/test` entry point after validating its pinned
commit.

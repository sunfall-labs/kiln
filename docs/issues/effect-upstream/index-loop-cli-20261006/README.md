# Combined frozen CLI, 2026-10-06

The one-worker pinned Rust 1.97.1 `mbx build --release -p kiln-cli --offline --jobs 1`
completed successfully in 217.58 seconds. The immutable binary has SHA-256
`5648787dead49c15a9a34839159fb683f566d41c867542e025c6bc17ec6a8269`.

The clean build-start source is `80f09d88926675f85f4144e172fe90a51a45e4c5`,
Git tree `48a6d446e485053a516c34fbc423276b21fc690c`. Only the documentation
commit `65db6122` landed during compilation; the crates/scripts/Cargo diff between
those revisions is empty. The receipt records both revisions rather than treating
this as a clean build-start from the later revision.

This combines the tested intrinsic lookup index, bounded known-call analysis and
bounded generator O1 tier. The combined backend test evidence records 339 passes,
zero failures and three ignored tests. Upstream performance and complete-suite
acceptance are still unproven; unchanged Workflow/Arbitrary interpreter and native
Sharding replays use this binary next.

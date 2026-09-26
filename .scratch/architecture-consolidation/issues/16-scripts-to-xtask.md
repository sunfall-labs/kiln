# 16. Move the gate scripts to `cargo xtask`

Status: ready-for-human

About 40 bash scripts, several with traps that `CLAUDE.md` documents: bash 3.2 losing `BASHPID`
and deleting the shared results directory, `tail` cutting the one summary line, and scripts that
must be executed rather than run with `bash`. A Rust `xtask` removes that whole class.

Needs a human decision first: which scripts move, and whether `mise run <task>` stays as the entry
point (recommended: yes, calling `cargo xtask`). Suggested first set: `gate.sh`, `native.sh`,
`thirdparty.sh`, `test262.sh`, and their result envelopes under `target/gate-logs/`.

`xtask` runs through `mbx`, not bare cargo, to keep the shared CPU budget.

## Acceptance

- The moved scripts are deleted; `mise run gate` output and `target/gate-logs` layout unchanged.
- The bash-version and "execute, don't `bash`" sections of `CLAUDE.md` are deleted.

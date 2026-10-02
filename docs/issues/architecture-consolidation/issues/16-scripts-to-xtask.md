# 16. Move the scripts to `cargo xtask`

Status: ready-for-agent

About 40 scripts (27 bash, 11 Python, 1 JavaScript), the bash ones with traps that `CLAUDE.md`
documents: bash 3.2 losing `BASHPID` and deleting the shared results directory, `tail` cutting the
one summary line, and scripts that must be executed rather than run with `bash`. A Rust `xtask`
removes that whole class.

Decided 2026-09-26 by the project owner: **all of the scripts move**, Python and JavaScript
included, not only the gate's, and `mise run <task>` stays the entry point, calling `cargo xtask`.
Start with `gate.sh`, `native.sh`, `thirdparty.sh` and `test262.sh`, since the gate depends on them,
then the rest in any order. Each script is deleted in the same change that replaces it.

`xtask` runs through `mbx`, not bare cargo, to keep the shared CPU budget.

## Acceptance

- `scripts/` is gone: every bash, Python and JavaScript script is an xtask subcommand. `mise run
  gate` output and the `target/gate-logs` layout are unchanged.
- `hk.pkl`'s shellcheck step is removed with the last script.
- The bash-version and "execute, don't `bash`" sections of `CLAUDE.md` are deleted.

# Verification, ADR 0046 state paragraph, NOTES entry

Status: done

After issues 01-05:

- `mbx test -p kiln-runtime -p kiln-interpreter -p kiln-cli --offline`
- `mbx build -p kiln-cli` then `target/debug/kiln test conformance/node --node` and
  `target/debug/kiln test conformance/server --node` (check the binary's mtime against
  `git log -1` first)
- `mbx clippy --workspace --offline`, `cargo fmt --check`
- `bash scripts/check-http-close-contract.sh` (mandatory: this changes HTTP close behaviour)
- `scripts/native.sh` (the archive changed; 0 diverged is the standing gate)

Then: replace the "Implementation state" paragraph of
`docs/adr/0046-http-connections-are-hyper-tasks-over-host-transports.md` with the finished
state, append a dated entry to `NOTES.md`, and record the after-numbers in `PRD.md`.

## Comments

- Done. Numbers in `PRD.md`. `mbx clippy --workspace` had one pre-existing *error* on main
  (`arithmetic_side_effects` on `epoch + started.elapsed()` in the archive's wall clock);
  changed to `saturating_add` so the gate passes. ADR 0046's state paragraph and a dated
  `NOTES.md` entry are in the final commit.

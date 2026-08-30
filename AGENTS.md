## Building

### Use `mbx`, not bare `cargo`

`mbx build`, `mbx test`, `mbx clippy` — it takes any Cargo command and passes the subcommand and
its arguments through unchanged, so `mbx test -p kiln-cli --offline` is exactly `cargo test -p
kiln-cli --offline` with two things added: one content-addressed cache shared across every
checkout, and a single CPU budget shared by concurrent invocations.

That second property is the one that matters here, because this repository is routinely built by
more than one agent at once. Bare `cargo` does not know the others exist and will happily take
every core; several of them will thrash a machine that `mbx` would have kept busy. Reach for bare
`cargo` only when something genuinely needs it, and say why.

`cargo fmt` has no wrapper — call it directly.

### The toolchain comes from mise

`rust@1.97.1` and `mbx` are pinned in the global mise config. Do not install Rust tooling by hand,
and do not stub a tool to test a fallback: put a stub in a directory and prepend it to `PATH` for
one command. (NOTES.md records ninety minutes lost to a stubbed `rustup` that resolved back into
itself.)

### When the disk fills

It has hit 100% and failed a link with `ld: write() failed, errno=28`. `mbx gc --max-size 20GiB`
prunes the shared store, `--dry-run` shows what it would remove first, and a missing cached object
is simply rebuilt when next needed. `.kiln/cache` is bounded separately by `kiln cache gc`.

## Agent skills

### Issue tracker

Issues and PRDs are repository-local Markdown under `.scratch/<feature>/`. See
`docs/agents/issue-tracker.md`.

### Triage labels

Use the standard five-role triage vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository with domain language in `CONTEXT.md` and decisions in
`docs/adr/`. See `docs/agents/domain.md`.

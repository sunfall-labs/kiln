# 12. `kiln explain <member>`

Status: ready-for-agent
Blocked by: 07

Answer "where does this live?" from the inventory:

```
$ kiln explain EventEmitter.prototype.on
owner:     bootstrap TypeScript (ADR 0060 kind 7)
source:    crates/kiln-builtins/src/node/events.ts:112
engines:   none (uses primitives: none)
refusal:   no
tests:     conformance/node/events-*.js (14), conformance/native/events-on.js
```

and, for a member still duplicated, every implementation with file and line. The triple
`EventEmitter` happened because nobody could see the other two copies.

## Acceptance

- Works for every member in `kiln-builtin-surface` and every provider export.
- Mentioned in each engine crate's README (04) as the first thing to run before adding a member.

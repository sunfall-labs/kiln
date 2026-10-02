# Whole-program facts review

This note classifies the whole-program facts changes against the current CLI86 source. The committed facts baseline remains unchanged; this review is evidence for a deliberate refresh after the final source snapshot is frozen.

## Evidence

- Source snapshot: `b425eb6465f1c4d397a10caaaee5909caf048414` (`b425eb64`).
- Committed baseline: `bench/results/facts-baseline.json`, SHA-256 `1efe5bdf111d46b76b080b251cc0e33d48fd71c45d386742d2003e234f4b4ec3`.
- Fresh CLI86 facts: `target/npm-compat-evidence/cli86-facts-current.json`, SHA-256 `bb8b6ce9ce70a72067e182d98bd9ea4184adb7071c554ef300a130c7cbe9cc62`. It is byte-identical to the reviewed CLI82 snapshot.
- Both files describe 31 targets: 14 benchmarks and 17 bundles. Fourteen targets differ from the committed baseline; the other 17 match.

## Reviewed differences

The changed targets are `benchmark:class-poly` and bundles `acorn`, `ajv`, `async-concurrency`, `chalk`, `datefns`, `dayjs`, `immer`, `marked`, `mobx`, `postcss`, `semver`, `uuid`, and `vue-reactivity`.

Most changed private-field facts reflect the addition of module identity to private-brand keys. The source commits are `637b434f` (qualify private fields by module identity) and `af7c23cb` (use project-relative private module identities). Baseline details such as `@@private:0:%brand` now include the encoded owning module path and ordinal. This explains the private-brand detail changes across class-poly and the listed bundles; it is an identity clarification, not evidence of changed user-visible behavior.

PostCSS also sees a `node:fs/promises` builtin shape change from the `readdir` addition in `7ff606fb` and `aacdb823`. In the facts diff, builtin function `#4` changes from `FileHandle` to `readdir`, shifting subsequent function records. The bundled PostCSS source itself is unchanged.

The generic `Call` site reductions caused by template-hole lowering are explained by `edc5ef51` (`Use abstract ToString for template interpolation`). Nonliteral template holes now lower to `Unary(ToString)` instead of a generic `Call`; the facts site inventory does not list that unary conversion as a call site. This preserves template interpolation’s string-hint coercion and avoids resolving a mutable public `String` binding.

| Target | Generic `Call` reductions from template holes | Source evidence |
| --- | ---: | --- |
| ajv | 129 | Nonliteral template holes in bundled AJV sources; independent of the two dropped `_compilations.delete` `CallMethod` cleanup sites. |
| chalk | 10 | Two ANSI open/close holes in `assembleStyles`, plus 1/2/4/1 holes in `wrapAnsi16`, `wrapAnsi256`, `wrapAnsi16m`, and `wrapUnderlineAnsi`. |
| datefns | 5 | `message` has five template holes; its explicit `token.toLowerCase()` method call remains. |
| dayjs | 1 | The `${a.toISOString()}` hole in `console.log` loses its generic conversion call; the explicit `String(a.valueOf())` call remains. |
| semver | 199 | Template holes in regex construction; 69 of these were in the `internal/re.js` factory, whose 67 computed reads and other site counts remain. |
| uuid | 2 | The `${offset}` and `${offset + 15}` holes in the bounds-error message. |

Acorn’s anonymous function `#151` changes return type from `Unknown` to `Object`, while its site list and `returned_sites: [0]` stay the same. Site 0 is `NewArray`. The matching source is `pp$7.parseBindingList`: it creates `elts` and returns that array. `Object` is consistent with the returned allocation. The precise analyzer change that strengthened this inference has not been identified.

A second pass compares sites by kind and detail after removing site numbers and normalizing module-qualified private keys. It finds three additional classes of change:

- `async-concurrency`, `marked`, `mobx`, and the builtin modules used by PostCSS replace class-field `SetPropertyStrict` sites with `DefineOwnProperty` sites. This is the explicit own-property initialization introduced by `6ac3f0fa`; names and counts pair exactly. Ordinary assignments remain assignments.
- AJV function #443 (`compileSchema`) loses two `GetProperty("_compilations")` and two `CallMethod("delete")` sites. The source has one `finally { this._compilations.delete(sch) }` at bundle line 2972; the missing sites are duplicated lowered-finally copies. Function #541 (`_loadSchema`) loses one of four `GetProperty("_loading")` sites. Its live reads/assignment are at lines 4432 and 4436; the removed base-object read is duplicated for `delete this._loading[ref]` in finally at line 4438.
- Vue function #11 (`refreshComputed`) loses one duplicated finally cleanup sequence: a `cleanupDeps` call and `computed2.flags` clear. Unnamed function #104 matches `ReactiveEffect.run` by its two `return this.fn()` sites and loses a duplicated cleanup sequence, including one `this.flags` clear and one repeated `process.env.NODE_ENV` guard read. These are two duplicated finally blocks; only the #11 write is literally `computed2.flags`.
- Commit `7c97cc39` computes CFG reachability before lowering, retains exception/suspension edges, and leaves unreachable blocks without operations. Its regression demonstrates that a block after `Throw` must not seed a reachable join with an empty frame. The cleanup facts above are consistent with removing dead lowered copies. Acorn’s return refinement is consistent with removing an empty unreachable join frame, but the exact causal link is not proven.
- Additional generic `Call` reductions include template-hole conversion sites. An independent AST count finds exactly 29 nonliteral untagged holes in async-concurrency, seven in Immer, and 37 in Marked, matching their generic-call reductions. AJV’s 129 template-hole reductions are an independent generic-`Call` count; its `_compilations.delete` differences are separately classified `CallMethod` sites. AJV, MobX, PostCSS and Vue also combine templates with duplicated cleanup blocks and imported builtin records; their aggregate reductions are not all one-to-one counts of template holes. For example, Vue has 12 source template holes and drops duplicated cleanup calls; its full generic-`Call` reduction is 15. These totals remain visible in the baseline diff rather than being assigned wholly to templates.

The reviewed causes cover the 14 changed targets, with Vue’s template-call split and Acorn’s return inference qualified as above. The facts baseline has not been edited. A deliberate refresh still requires the final frozen source run and review of its exact JSON diff; this review does not update or weaken any conformance floor.

## Deliberate baseline update

The CLI87 final-source snapshot (`d6627566`) is byte-identical to the reviewed CLI82 and CLI86 snapshots (SHA-256 `bb8b6ce9ce70a72067e182d98bd9ea4184adb7071c554ef300a130c7cbe9cc62`). It includes the published GUI integration and the reserved builtin-loader repair. The committed facts baseline is updated to this reviewed 31-target snapshot; the fourteen changed targets are classified above. No conformance passing floor is changed.

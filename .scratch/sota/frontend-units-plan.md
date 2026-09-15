# Frontend and remaining language identities — implementation handoff

Status: pending Luna implementation after JSON work.

## Ownership

Coordinate before editing frontend-oxc, kiln-hir, kiln-ir, graph/module adapters, and interpreter source/name seams. Native agent owns backend-C migration; property agent owns the property-family interpreter edits. Use a new owned worktree and append commits.

## Required slices

1. Preserve quoted-key inferred function/class/accessor names as StringUnits through HIR/IR and function metadata. Written identifier bindings can remain scalar strings because ECMAScript identifiers exclude lone surrogates. Do not convert arbitrary quoted keys to scalar names.
2. Auto-accessor private backing slots currently use `%accessor:{lossy key}`. Distinct lone-surrogate keys can collide. Generate unique structural private slot names (source/member identity), and retain the user-visible property name separately in units.
3. Module exported/imported string names and namespace enumeration must preserve units. Source paths/specifiers need their actual host encoding policy, distinct from language export names. Audit namespace_entries, ShadowRealm importValue, export bindings, star expansion, and synthetic graph constants.
4. Dynamic eval/Function/ShadowRealm source must not silently use to_lossy_string. Oxc parser accepts &str, so inspect ecosystem tokenizer/AST APIs for a narrow source adapter preserving exact string/template/regexp contents and invalid-source errors. Avoid writing another JavaScript lexer. Keep tagged template raw contents and function-source text distinct from cooked strings. If a complete boundary cannot yet be implemented, report the precise unsupported case for reviewer decision; do not substitute U+FFFD or empty code.
5. Preserve symbol descriptions and function-name reflection across both runtimes. Coordinate native counterpart changes with its owner.

## Evidence

Node-oracle fixtures should print charCodeAt arrays, not rendered strings. Cover high/low lone surrogates, actual U+FFFD, astral pairs, escaped and raw forms, distinct accessor backing state, inferred method/class names, exported string names, and dynamic source. Add focused crate tests and meaningful native probes, then leave full gate to frozen integration.

## Known current locations

- frontend-oxc/src/lib.rs: name_anonymous_function calls still pass key.to_lossy_string; Class.inferred_name remains Option<String>.
- frontend auto-accessor lowering: storage = format!("%accessor:{}", key.to_lossy_string()).
- kiln-hir ImportName/ExportName currently Name(String); public export-name APIs return scalar strings.
- kiln-interpreter namespace_entries converts property names lossily; eval/ShadowRealm source calls still convert lossily.
- Property agent separately owns string_constant migration for static-key instructions.

## Module-name scope correction

ModuleExportName StringLiteral has an early error when IsStringWellFormedUnicode is false:
https://tc39.es/ecma262/2025/multipage/ecmascript-language-scripts-and-modules.html#sec-module-semantics-static-semantics-early-errors
Root confirmed Node rejects `export {x as "\\ud800"}` and accepts U+FFFD. Therefore item 3 does
not require wholesale module-name type migration: preserve validated scalar storage and test
the early-error invariant. Dynamic arbitrary lookup names must not be lossily aliased to valid
U+FFFD exports; convert checked or compare units at the lookup boundary. Host synthetic modules
need their own explicit contract if arbitrary-unit exports are permitted.

## Dynamic source ecosystem lead (investigate, not an approved parser replacement)

Boa parser exposes Source::from_utf16 and a generic Lexer over ReadChar:
https://docs.rs/boa_parser/latest/src/boa_parser/source/mod.rs.html
https://docs.rs/boa_parser/latest/boa_parser/lexer/struct.Lexer.html
This may help classify/rewrite rare raw-surrogate source spans without writing a JavaScript lexer.
A standalone lexer still has contextual regex/division/template limitations; inspect the actual
APIs before proposing it. Avoid a second execution engine or a wholesale replacement of Oxc.
An adapter must preserve cooked literals, tagged template raw contents, source reflection and
syntax errors independently. Normal wellformed source should keep the existing borrowed fast path.

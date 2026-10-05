# Formatter `Function#toString` transform evidence

This note records the source transformation behind the pinned upstream
`Formatter > format > function` assertion. It does not change the test, compiler,
or direct-TypeScript source contract.

## Inputs and toolchain

- Pinned Effect commit: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`
- Test: `source/packages/effect/test/Formatter.test.ts`, lines 150–160
- Input SHA-256: `9d816f3751b14da252a9fa3c5082668a08d8d2b683bd4878ae5da3d87f503399`
- Node: `v26.8.1`
- Vitest: `5.0.3`
- Vite: `8.3.2`
- Rolldown: `1.2.12`
- Bun: `1.4.2`

The pinned Vitest setup sets `viteConfig.oxc.target` to `node22` when unset
(`node_modules/vitest/dist/chunks/index.DpLw24bj.js`, around lines 7999–8010).
Vite's Oxc transform plugin passes TypeScript module source through
`transformWithOxc`, which calls `transformSync` and returns JavaScript plus a
sourcemap (`node_modules/vite/dist/node/chunks/node.js`, around lines 7556–7566
and 7649–7655).

## Captured transform

I invoked the installed Vite `transformWithOxc` helper on the complete pinned
test file with `lang: "ts"` and `target: "node22"`. The transformed JavaScript
`.code` SHA-256 was:

```
676d20ba9a49a4759489ec5e4e2b2a40ec48ee67da7749b39d0fb05b894acfc2
```

The relevant transformed code contains actual tab characters in its indentation:

```js
strictEqual(format(() => {
				return 1;
			}), `() => {\n\t\t\t\treturn 1;\n\t\t\t}`);
```

That arrow is parsed from transformed JavaScript whose body has a semicolon and
tab indentation. Its reflected source is therefore the string the assertion
expects. The authored TypeScript at lines 155–159 has ten spaces before
`return 1`, eight before the closing brace, and no semicolon.

Reproduction command, run from the pinned workspace root:

```sh
node --input-type=module - <<'JS'
import fs from 'node:fs';
import crypto from 'node:crypto';
import { transformWithOxc } from 'vite';
const file = `${process.cwd()}/source/packages/effect/test/Formatter.test.ts`;
const input = fs.readFileSync(file, 'utf8');
const result = await transformWithOxc(input, file,
  { lang: 'ts', target: 'node22' }, undefined, {});
console.log('input sha256', crypto.createHash('sha256').update(input).digest('hex'));
console.log('output sha256', crypto.createHash('sha256').update(result.code).digest('hex'));
const at = result.code.indexOf('strictEqual(format(() => {', 0);
console.log(result.code.slice(at, result.code.indexOf(');', at) + 2));
JS
```

The helper was run on the whole file, not on a reconstructed callback. The full
output was not retained; its digest and the exact function/assertion slice above
are the compact evidence.

## Direct-runtime controls

With a temporary `.mts` file using the same nested body indentation and a type
annotation on the binding, direct Node strip-types and Bun returned:

```text
Node 26.8.1 --experimental-strip-types:
"() => {\n          return 1\n        }"

Bun 1.4.2:
"() => {\n  return 1;\n}"
```

Node preserves the authored source slice. Bun emits its own compiled function
text. Neither result is the Vite/Oxc transformed slice. These controls match the
existing exact-case classification in
`conformance/effect-upstream/node-vitest-comparison-gaps.json`; the case remains
a mismatch in direct-engine reports.

## Frontend fit and tradeoff

The current frontend parses the original TypeScript source with Oxc, lowers the
supported TypeScript syntax into kiln HIR, and retains that original buffer for
source spans. The HIR and IR contracts explicitly make function spans index
their retained source buffer (`crates/syntax/kiln-hir/src/lib.rs` around line
114; `crates/compile/kiln-ir/src/lib.rs` around lines 605 and 745). Interpreter
and native closures return those spans from that buffer. This matches the
existing direct-TypeScript design.

A static per-module TS-to-JS stage could also obey the parser-input invariant,
but only if transformed JavaScript becomes the actual parser input. In that
design, `Program.source` would own the transformed buffer used by function
spans and `Function#toString`; the module would separately retain original TS
and a source map for diagnostics. Transform version/options and output bytes
would need to participate in module/cache identity. This would be a real
frontend mode change shared by interpreter and native, not a printer rule in
`Function#toString`.

The current frontend has Oxc parser, semantic, transformer, and minifier
dependencies, but no Oxc code generator dependency. Its transformer is applied
to JSX-bearing syntax in the parse path; ordinary TS types are erased by kiln's
custom HIR lowering, which keeps original spans. Producing Vite-compatible
module text would therefore need an explicit code-generation stage and
transform configuration, then composition of transformed-to-original source
maps. It cannot be achieved by changing only the source slice attached to a
closure.

Existing checked-in `Function#toString` conformance focuses on exact source
spelling for JavaScript syntax, including methods, arrows, comments, and dynamic
Function-constructor source (`conformance/native/function-to-string.js` and
`conformance/native/dynamic-function-string-escapes.js`). No current test
requires TypeScript erasure in the returned source. If a Vite-compatible mode
is adopted, add a paired contract: a direct-source TS module reflects its
original parser input, while a transformed module reflects the exact generated
JS slice and diagnostics still map to original TS coordinates.

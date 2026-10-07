# @kiln/types

TypeScript declarations for what kiln adds to JavaScript: its own modules and its built-in
extensions. The modules themselves are part of the engine, imported by their `kiln:` names; this
package only describes them, as `@types/node` describes `node:*`.

| Module | What it is |
| --- | --- |
| `kiln:gui`, `kiln:gui/jsx-runtime` | native GUI widgets, layout and JSX |
| `kiln:actors` | classes that run in a realm of their own, called through a mailbox |
| `kiln:shared` | frozen graphs every realm can read without a copy |
| `kiln:process` | `run(command, args, options)` |
| `Array` / typed arrays | `parallelMap`, `parallelFilter`, `parallelFlatMap`, `parallelSort`, `parallelReduce` |

```jsonc
// tsconfig.json
{
  "compilerOptions": {
    "types": ["@kiln/types"],
    "jsxImportSource": "kiln:gui" // only for kiln:gui JSX
  }
}
```

```ts
import { spawnActor } from "kiln:actors";
import type { Counter } from "./counter.ts";

const owner = await spawnActor<Counter>({
  module: new URL("./counter.ts", import.meta.url),
  export: "Counter",
  args: [0],
});
console.log(await owner.ref.call("increment", [1]));
await owner.stop();
```

Every `kiln:` module is an extension: `--no-kiln-extensions` (or `KILN_EXTENSIONS=0`) removes it,
and importing it then fails by name. Node does not have these modules.

The declarations are generated from `crates/semantics/kiln-builtins/src`; edit the source and run
`scripts/gen-kiln-types.sh`, never these files. `mise run kiln-types` checks they are current.

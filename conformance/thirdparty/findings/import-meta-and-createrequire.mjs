// The preamble every ESM-for-node bundle carries.
//
// esbuild, tsup and rollup all emit `createRequire(import.meta.url)` when a package they bundle
// reaches a Node builtin. Kiln refuses `import.meta` in the frontend, so the whole bundle fails
// to compile; and `createRequire` itself, reached any other way, fails at run time. Together
// these mean the ESM half of the Node ecosystem's build output cannot be loaded at all -- which
// is why the four heaviest probes in this directory are bundled as CommonJS instead.
//
// node:  import-meta object string / createRequire function / req function / path function object
// kiln:  refused: "unsupported expression: import.meta expression"
//        and, reached without import.meta:
//        "node:module createRequire requires the captured graph resolution index"
import { createRequire } from "node:module";
console.log("createRequire", typeof createRequire);
console.log("import-meta", typeof import.meta, typeof import.meta.url);
const req = createRequire("/tmp/x.js");
console.log("req", typeof req);
console.log("path", typeof req("node:path").join, typeof req("path"));

# Historical non-pass inventory

Status: in-progress

This inventory preserves the committed 2026-09-30 historical diagnostics. Fresh subset evidence is recorded in ../results/targeted-node-mode-2026-09-30.json (14 of 26 pass, 334 deferred); it does not establish the status of unselected rows.

| Package | Status | Historical diagnostic |
| --- | --- | --- |
| semver | wrong-output | (18s) dur=18s |
| ansi-styles | compile-error | (3s) exit=1; stderr: kiln: uncaught SyntaxError: Invalid regular expression at module=node_modules/ansi-styles/index.js instruction=342 operation=Call detail=Call invocation threw function=assembleSty |
| chalk | runtime-error | (10s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a function at module=tests/chalk.test.mjs instruction=10 operation=CallMethod(red) detail=non-callable callee kind; |
| picomatch | runtime-error | (13s) exit=1; stderr: kiln: native backend refusal: module=node_modules/picomatch/lib/picomatch.js instruction=99 operation=SetPropertyStrict(state) detail=a property write to a value whose store noth |
| glob | runtime-error | (1s) exit=1; stderr: kiln: module /home/hatch/workspace/kiln-npm-eval/node_modules/glob/dist/esm/index.min.js imports missing export `readdir` from /home/hatch/workspace/kiln-npm-eval/.kiln/builtins/f |
| zod | runtime-error | (1s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/zod/v4/locales/index.js has an unresolved import source `./ar.js`; |
| iconv-lite | runtime-error | (16s) exit=1; stderr: kiln: native backend refusal: module=node_modules/safer-buffer/safer.js instruction=33 operation=Call detail=a built-in with no call behaviour in the native backend: Buffer recei |
| typescript | runtime-error | (75s) exit=137; stderr: |
| string_decoder | wrong-output | (3s) dur=3s |
| @babel/traverse | compile-error | (194s) exit=1; stderr: kiln: native toolchain failed: C compilation failed (status Some(1)): In file included from kiln_module_path_387520abd055df6472810f22e1ae0563_chunk_0.c:2:;kiln_runtime_prelude.h |
| @babel/core | runtime-error | (1s) exit=1; stderr: kiln: module /home/hatch/workspace/kiln-npm-eval/node_modules/empathic/package.mjs imports missing export `env` from /home/hatch/workspace/kiln-npm-eval/.kiln/builtins/process.bui |
| camelcase | runtime-error | (1s) exit=1; stderr: kiln: native backend refusal: module=node_modules/camelcase/index.js instruction=3 operation=CallMethod(toLocaleLowerCase) detail=a String.prototype member this backend has not im |
| form-data | runtime-error | (2s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| http-errors | runtime-error | (5s) exit=1; stderr: kiln: uncaught TypeError: Cannot read properties of undefined at module=node_modules/depd/index.js instruction=34 operation=CallMethod(slice) detail=CallMethod property lookup thr |
| fresh | wrong-output | (2s) dur=2s |
| jose | runtime-error | (9s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a constructor at module=tests/jose.test.mjs instruction=0 operation=FunctionBody detail=generated function body refused before an inst |
| express | runtime-error | (2s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| prettier | timeout | (600s) kiln run --native exceeded 600s |
| axios | runtime-error | (0s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/axios/lib/helpers/toFormData.js imports missing export `default` from /home/hatch/workspa |
| prop-types | runtime-error | (6s) exit=1; stderr: kiln: native backend refusal: module=node_modules/object-assign/index.js instruction=21 operation=New detail=a primitive wrapper constructor function=String receiver_tag=8; |
| htmlparser2 | compile-error | (0s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/htmlparser2/dist/index.js has an unresolved import source `./Parser.js`; |
| @noble/hashes | runtime-error | (12s) exit=1; stderr: kiln: native backend refusal: module=node_modules/@noble/hashes/utils.js instruction=11 operation=GetProperty(fromHex) detail=a member the language puts on this built-in and this |
| fflate | runtime-error | (17s) exit=1; stderr: kiln: uncaught Error: date not in range 1980-2099 at module=node_modules/fflate/lib/node.cjs instruction=174 operation=Call detail=Call invocation threw function=err receiver_tag |
| fast-xml-parser | compile-error | (27s) exit=1; stderr: kiln: native backend refusal: module=node_modules/fast-xml-parser/lib/fxp.cjs instruction=931 operation=SetComputedStrict detail=a property write to a value whose store nothing e |
| terser | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| p-map | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/p-map/index.js instruction=125 operation=GetComputed detail=@@asyncIterator on a synchronous generator receiver_tag=12; |
| marked | runtime-error | (27s) exit=1; stderr: kiln: uncaught Error: Cannot find module 'marked' at module=tests/marked.test.js instruction=6 operation=Call detail=Call invocation threw receiver_tag=26; |
| @tanstack/query-core | runtime-error | (28s) exit=1; stderr: FAILED {}; |
| jsonc-parser | compile-error | (4s) exit=1; stderr: kiln: uncaught Error: Cannot find module './impl/format' at module=node_modules/jsonc-parser/lib/umd/main.js instruction=78 operation=Call detail=Call invocation threw receiver_ta |
| lz-string | wrong-output | (5s) dur=5s |
| safe-stable-stringify | runtime-error | (6s) exit=1; stderr: kiln: uncaught TypeError: get %TypedArray%.prototype[@@toStringTag] needs a typed array receiver at module=node_modules/safe-stable-stringify/index.js instruction=2 operation=Call |
| bluebird | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: a direct `eval` call in the native backend; |
| cac | wrong-output | (8s) dur=8s |
| h3 | runtime-error | (20s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a function at module=tests/h3.test.mjs instruction=6 operation=Call detail=non-callable callee kind; |
| uglify-js | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/uglify-js/tools/node.js instruction=25 operation=CallMethod(resolve) detail=own properties are not modelled on this callable rece |
| quick-lru | runtime-error | (5s) exit=1; stderr: kiln: native backend refusal: module=node_modules/quick-lru/index.js instruction=26 operation=DefinePrivate detail=a method definition on a value with no property store; |
| immutable | runtime-error | (24s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a function at module=tests/immutable.test.js instruction=152 operation=CallMethod(setIn) detail=non-callable callee kind; |
| util | runtime-error | (3s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a function at module=tests/util.test.js instruction=63 operation=CallMethod(isDeepStrictEqual) detail=non-callable callee kind; |
| luxon | runtime-error | (38s) exit=1; stderr: kiln: native backend refusal: module=node_modules/luxon/build/node/luxon.js instruction=8 operation=GetProperty(DateTimeFormat) detail=a member the language puts on this built-in |
| moment | runtime-error | (19s) exit=1; stderr: kiln: out of memory; |
| @noble/curves | runtime-error | (26s) exit=1; stderr: kiln: native backend refusal: module=node_modules/@noble/hashes/utils.js instruction=11 operation=GetProperty(fromHex) detail=a member the language puts on this built-in and this |
| hookable | runtime-error | (3s) exit=1; stderr: kiln: uncaught Error: Cannot find module 'hookable' at module=tests/hookable.test.js instruction=4 operation=Call detail=Call invocation threw receiver_tag=26; |
| @reduxjs/toolkit | runtime-error | (30s) exit=1; stderr: kiln: native backend refusal: module=node_modules/@reduxjs/toolkit/dist/cjs/redux-toolkit.development.cjs instruction=84 operation=CallMethod(assign) detail=Object.assign target  |
| underscore | runtime-error | (9s) exit=1; stderr: kiln: uncaught SyntaxError: Unexpected token in expression at module=node_modules/underscore/underscore-node-f.cjs instruction=4 operation=CallMethod(call) detail=Call invocation  |
| lodash.clonedeep | runtime-error | (5s) exit=1; stderr: kiln: native backend refusal: module=node_modules/lodash.clonedeep/index.js instruction=4 operation=Call detail=an own-key listing of a value with no property store receiver_tag=1 |
| boxen | runtime-error | (1s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/boxen/index.js has an unresolved import source `cli-boxes`; |
| winston | runtime-error | (60s) exit=1; stderr: kiln: uncaught TypeError: Class constructor Stream cannot be invoked without 'new' at module=node_modules/winston/node_modules/readable-stream/lib/_stream_readable.js instruction |
| validator | runtime-error | (37s) exit=1; stderr: kiln: uncaught ReferenceError: toString is not defined; |
| xmlbuilder2 | runtime-error | (113s) exit=1; stderr: kiln: uncaught TypeError: class heritage is not a constructor or null at module=node_modules/@oozcitak/dom/lib/dom/ElementImpl.js instruction=33 operation=Call detail=Call invoc |
| @paralleldrive/cuid2 | runtime-error | (11s) exit=1; stderr: kiln: native backend refusal: module=node_modules/@noble/hashes/utils.js instruction=11 operation=GetProperty(fromHex) detail=a member the language puts on this built-in and this |
| assert | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| title-case | runtime-error | (1s) exit=1; stderr: kiln: uncaught TypeError: Cannot read properties of undefined at module=tests/title-case.test.mjs instruction=2 operation=Call detail=Call invocation threw function=titleCase rece |
| turndown | runtime-error | (74s) exit=1; stderr: kiln: uncaught TypeError: a object is not a constructor at module=node_modules/turndown/lib/turndown.cjs.js instruction=2 operation=CallMethod(createDocument) detail=CallMethod i |
| fast-json-patch | runtime-error | (0s) exit=1; stderr: kiln: could not resolve `./src/core` imported by /home/hatch/workspace/kiln-npm-eval/node_modules/fast-json-patch/index.ts: could not inspect target /home/hatch/workspace/kiln-npm |
| @iarna/toml | runtime-error | (0s) exit=1; stderr: kiln: native emission failed: unsupported native emission: a direct `eval` call in the native backend; |
| eta | compile-error | (5s) exit=1; stderr: kiln: uncaught SyntaxError: Unexpected token in expression at module=node_modules/eta/dist/index.mjs instruction=59 operation=CallMethod(call) detail=CallMethod invocation threw f |
| koa | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: a direct `eval` call in the native backend; |
| just-diff | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/just-diff/index.mjs instruction=73 operation=Call detail=Object() of a primitive this backend has no wrapper for receiver_tag=8; |
| jsbi | wrong-output | (9s) dur=9s |
| hash-sum | wrong-output | (2s) dur=2s |
| @date-fns/utc | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/@date-fns/utc/date/mini.js instruction=11 operation=GetComputed detail=a member the language puts on this built-in and this backe |
| arktype | runtime-error | (3s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `nodeClassesByKind`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/@ark/schema |
| showdown | runtime-error | (18s) exit=1; stderr: kiln: an argument was not valid; |
| random-seed | runtime-error | (3s) exit=1; stderr: kiln: uncaught TypeError: a object is not a function at module=node_modules/random-seed/index.js instruction=61 operation=Call detail=non-callable callee kind receiver_tag=9; |
| change-case | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/change-case/dist/index.js instruction=2 operation=CallMethod(toLocaleLowerCase) detail=a String.prototype member this backend has |
| cli-table3 | wrong-output | (10s) dur=10s |
| commander | runtime-error | (0s) exit=1; stderr: kiln: module /home/hatch/workspace/kiln-npm-eval/node_modules/commander/lib/help.js imports missing export `stripVTControlCharacters` from /home/hatch/workspace/kiln-npm-eval/.kil |
| common-tags | runtime-error | (13s) exit=1; stderr: kiln: uncaught TypeError: a object is not a function at module=tests/common-tags.test.js instruction=34 operation=Call detail=non-callable callee kind receiver_tag=9; |
| consola | runtime-error | (15s) exit=1; stderr: kiln: native backend refusal: module=@kiln/runtime-bootstrap.ts instruction=19 operation=GetProperty(DateTimeFormat) detail=a member the language puts on this built-in and this b |
| core-js | runtime-error | (22s) exit=1; stderr: kiln: native backend refusal: module=node_modules/core-js/internals/indexed-object.js instruction=2 operation=Call detail=Object() of a primitive this backend has no wrapper for  |
| cron-parser | compile-error | (13s) exit=1; stderr: kiln: uncaught TypeError: Cannot read private member #values from an object whose class did not declare it at module=node_modules/cron-parser/dist/fields/CronSecond.js instructio |
| css-tree | runtime-error | (1s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/css-tree/lib/index.js has an unresolved import source `./version.js`; |
| cssom | compile-error | (10s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a constructor at module=tests/cssom.test.js instruction=10 operation=CallMethod(parse) detail=CallMethod invocation threw function=pa |
| csv-parse | compile-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `transform`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/csv-parse/lib/api/i |
| culori | compile-error | (3s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `default`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/culori/src/parse.js`  |
| d3-format | runtime-error | (5s) exit=1; stderr: kiln: uncaught Error: invalid format:  >-,.0f at module=node_modules/d3-format/src/locale.js instruction=50 operation=Call detail=Call invocation threw function=formatSpecifier re |
| d3-interpolate | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `default`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/d3-interpolate/src/va |
| d3-scale | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `default`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/d3-interpolate/src/va |
| date-fns-tz | runtime-error | (81s) exit=1; stderr: kiln: native backend refusal: module=node_modules/date-fns-tz/dist/cjs/_lib/tzTokenizeDate/index.js instruction=62 operation=GetProperty(DateTimeFormat) detail=a member the langu |
| deep-equal | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| dinero.js | runtime-error | (0s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/dinero.js/dist/esm/index.js has an unresolved import source `./currencies/index.js`; |
| dotenv | runtime-error | (6s) exit=1; stderr: kiln: native backend refusal: module=node_modules/dotenv/dist/index.cjs instruction=89 operation=GetProperty(main) detail=own properties are not modelled on this callable receiver |
| effect | runtime-error | (573s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a function at module=node_modules/effect/dist/cjs/Schema.js instruction=5088 operation=CallMethod(annotations) detail=non-callable c |
| ejs | compile-error | (9s) exit=1; stderr: kiln: uncaught SyntaxError: Unexpected token in expression at module=node_modules/ejs/lib/cjs/ejs.js instruction=41 operation=CallMethod(apply) detail=Call invocation threw functi |
| entities | runtime-error | (1s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/entities/dist/decode.js has an unresolved import source `./generated/decode-data-xml.js`; |
| espree | runtime-error | (0s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/espree/espree.js has an unresolved import source `eslint-visitor-keys`; |
| fastify | runtime-error | (127s) exit=1; stderr: kiln: native backend refusal: module=node_modules/thread-stream/index.js instruction=341 operation=New detail=new on a callee that is not a generated function function=Finalizat |
| handlebars | runtime-error | (29s) exit=1; stderr: kiln: native backend refusal: module=node_modules/handlebars/lib/index.js instruction=43 operation=GetProperty(extensions) detail=own properties are not modelled on this callable |
| hono | runtime-error | (12s) exit=1; stderr: kiln: uncaught TypeError: Cannot write private member #path to an object whose class did not declare it at module=tests/hono.test.mjs instruction=11 operation=CallMethod(get) det |
| itty-router | wrong-output | (5s) dur=5s |
| joi | runtime-error | (67s) exit=1; stderr: kiln: uncaught TypeError: Cannot read properties of undefined at module=node_modules/@hapi/address/dist/index.js instruction=67 operation=Call detail=Call invocation threw receiv |
| js-base64 | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/js-base64/base64.js instruction=227 operation=GetProperty(fromBase64) detail=a member the language puts on this built-in and this |
| json-stable-stringify | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| jsonwebtoken | runtime-error | (11s) exit=1; stderr: kiln: uncaught TypeError: Cannot read properties of undefined; |
| kind-of | runtime-error | (7s) exit=1; stderr: kiln: native backend refusal: module=node_modules/kind-of/index.js instruction=2 operation=GetProperty(length) detail=a RegExp.prototype member this backend has not implemented re |
| liquidjs | runtime-error | (53s) exit=1; stderr: kiln: native backend refusal: module=node_modules/liquidjs/dist/liquid.node.js instruction=6 operation=GetProperty(DateTimeFormat) detail=a member the language puts on this built |
| markdown-it | runtime-error | (29s) exit=1; stderr: kiln: uncaught ReferenceError: atob is not defined at module=node_modules/markdown-it/dist/markdown-it.cjs.js instruction=871 operation=Call detail=Call invocation threw function |
| mathjs | runtime-error | (10s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `hasOwnProperty`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/mathjs/lib/es |
| micromatch | runtime-error | (15s) exit=1; stderr: kiln: native backend refusal: module=node_modules/micromatch/node_modules/picomatch/lib/picomatch.js instruction=99 operation=SetPropertyStrict(state) detail=a property write to  |
| minimatch | runtime-error | (11s) exit=1; stderr: kiln: native backend refusal: module=node_modules/minimatch/dist/esm/ast.js instruction=314 operation=CallMethod(assign) detail=Object.assign target must be an object, an array o |
| nanoid | runtime-error | (3s) exit=1; stderr: kiln: native backend refusal: module=node_modules/nanoid/index.js instruction=22 operation=CallMethod(getRandomValues) detail=a member the language puts on this built-in and this  |
| nunjucks | runtime-error | (23s) exit=1; stderr: kiln: uncaught TypeError: Class constructor EventEmitter cannot be invoked without 'new' at module=node_modules/nunjucks/src/object.js instruction=6 operation=CallMethod(call) de |
| object-hash | runtime-error | (4s) exit=1; stderr: kiln: native backend refusal: module=node_modules/object-hash/index.js instruction=151 operation=CallMethod(keys) detail=an own-key listing of a value with no property store; |
| pino | runtime-error | (4s) exit=1; stderr: kiln: native backend refusal: module=node_modules/thread-stream/index.js instruction=341 operation=New detail=new on a callee that is not a generated function function=Finalizatio |
| qs | runtime-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| readable-stream | runtime-error | (22s) exit=1; stderr: kiln: uncaught TypeError: Class constructor EventEmitter cannot be invoked without 'new' at module=node_modules/readable-stream/lib/internal/streams/legacy.js instruction=3 opera |
| recast | runtime-error | (46s) exit=1; stderr: kiln: uncaught TypeError: a undefined is not a function at module=node_modules/ast-types/lib/types.js instruction=121 operation=Call detail=non-callable callee kind; |
| sanitize-html | compile-error | (0s) exit=1; stderr: kiln: native module graph failed: module /home/hatch/workspace/kiln-npm-eval/node_modules/htmlparser2/dist/index.js has an unresolved import source `./Parser.js`; |
| simplex-noise | wrong-output | (2s) dur=2s |
| smol-toml | compile-error | (1s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `parseArray`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/smol-toml/dist/str |
| source-map-js | compile-error | (2s) exit=1; stderr: kiln: uncaught SyntaxError: Unexpected token in expression at module=node_modules/source-map-js/lib/quick-sort.js instruction=39 operation=Call detail=Call invocation threw functi |
| stream-browserify | runtime-error | (11s) exit=1; stderr: kiln: uncaught TypeError: Class constructor Stream cannot be invoked without 'new' at module=node_modules/stream-browserify/node_modules/readable-stream/lib/_stream_writable.js i |
| svelte | runtime-error | (47s) exit=1; stderr: kiln: native emission failed: unsupported native emission: `has_await_expression`, imported across a module cycle before `/home/hatch/workspace/kiln-npm-eval/node_modules/svelte/ |
| tapable | wrong-output | (7s) dur=7s |
| through2 | runtime-error | (4s) exit=1; stderr: kiln: uncaught TypeError: Class constructor EventEmitter cannot be invoked without 'new' at module=node_modules/readable-stream/lib/internal/streams/legacy.js instruction=3 operat |
| toml | runtime-error | (10s) exit=1; stderr: kiln: native backend refusal: module=node_modules/toml/lib/parser.js instruction=49 operation=CallMethod(call) detail=an own-property question about a built-in member the surface |
| type-detect | runtime-error | (2s) exit=1; stderr: kiln: native backend refusal: module=node_modules/type-detect/type-detect.js instruction=177 operation=GetComputed detail=a RegExp.prototype member this backend has not implemente |
| url | runtime-error | (0s) exit=1; stderr: kiln: native emission failed: unsupported native emission: the `Eval` builtin in the native backend; |
| ws | runtime-error | (12s) exit=1; stderr: kiln: uncaught Error: Cannot find module 'tls' at module=node_modules/ws/lib/websocket.js instruction=184 operation=Call detail=Call invocation threw receiver_tag=26; |
| yargs | runtime-error | (0s) exit=1; stderr: kiln: native Node execution does not yet materialize the `node:fs` export `writeFile`; |

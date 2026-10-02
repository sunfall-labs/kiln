# Validation history and current npm evidence

Status: in-progress

The following sections retain immutable historical pins. The latest section records current source and validation; earlier unverified or blocked statements describe their named checkpoint.

Source: `08d51089ce60414244dc1a57d6c1b2de487b3a33`; immutable CLI SHA-256: `3b36b3209d640b79d0e6265a169fb021661da80cbeeeec098207d4a0932dea22`.

Release build passed with `mbx build --release --locked -p kiln-cli --offline --jobs 2` in the managed checkout. The raw successful log is `target/npm-compat-evidence/build-thirty-second-authority-retry.log`; preceding failed attempts are retained separately.

The first native DirectEval probe was refused during graph lowering (0 matched, 1 refused), despite the existing tail emitter. The non-eval Object.prototype membership probe then failed generated-C compilation (0 matched, 1 invalid). It identified a missing RegExp slicing declaration, an incorrect HTTP layout typedef, and a boundary/machine value mismatch in dynamic capture strictness. These are build failures, not conformance passes. Source fixes await the next batched CLI validation.

Node preflight passed `directeval-function-expression.mjs`, `directeval-named-function-binding.cjs`, and `regexp-legacy-capture-routes.mjs`. The HTTP upgrade preflight failed with `listen EPERM 127.0.0.1:19838` under the current restricted sandbox. This is an environment blocker, not evidence of correct HTTP behavior; the socket probes and ws consumer remain unverified. No package batch, full corpus, final gate, or push has run for CLI32.

## CLI33 follow-up

Source `28ed4418e93c34c1c53d2fb197289d05cd2a3825`, binary SHA-256 `c324758c7dd79da1ccaccc1e6e953a84bb6d63ef838b8190d34a5525b835424a`: release build and generated-C compilation completed, but both first-pair probes refused during shared function-kind bootstrap. The TS bootstrap incorrectly passed callable closures where the native guard requires the ordinary prototype objects that Node exposes. That source fix awaits validation.

A cheap Node/interpreter oracle batch selected 22 non-socket regression fixtures: 19 matched, 3 differed (`function-kinds.js`, `directeval-function-capture-scope.mjs`, `directeval-named-function-binding.cjs`). The interpreter conflates AsyncFunction and GeneratorFunction identities, ignores explicit null function prototypes in reads, and loses immutable binding authority during eval/function-expression capture. Luna source slices are addressing those issues. Machine evidence: `../results/cli33-node-interpreter-oracle.json`. This is targeted fixture coverage, not a native or npm corpus result.

## CLI34 and CLI35 follow-up

CLI34 source `eed20ab6` (fixture-only assertions added at `d1285d40`), SHA-256 `069cd467c0d1099a766f79cc97fbb926114b93e3b344bd4028a2fb2bebe2e55d`, built successfully. The first native trio matched Object prototype membership and DirectEval; an arrow function's assigned prototype descriptor was incorrectly absent. CLI34's Node/interpreter oracle selected 24 probes: 20 matched, three runtime issues remained and one new module fixture had an incorrect strict-mode expectation. Raw output is retained under `target/npm-compat-evidence/cli34-oracle`; the strict expectation is corrected separately.

CLI35 source `0cb67b5b`, SHA-256 `c52f508841d64b1040c663a1237adf470e010a27de9aa33f435210579bc6aeaf`, built successfully. Native ten-probe batch: 7 matched, 1 refused (SharedArrayBuffer constructor's undeclared Math dependency), 2 diverged (computed namespace reads and String.matchAll's ignored bootstrap override). Targeted npm coverage: 16 selected, 345 deferred, 361 discovered; 5 passed, 11 runtime errors. Koa, Showdown, Fastify, qs and url passed. Exact package results are retained in `../results/targeted-thirty-fifth.json`; full raw output remains under `conformance/npm-compat/results/raw/20261001T023521Z.9`.

Architecture follow-up targets shared authority rather than package exceptions: descriptor overlays must preserve intrinsic attributes; namespace reads must unwrap live cells on every access path; bootstrap method overrides must precede native tables; native retention must include the dependencies of public TS implementations; binding write authority must survive modules without rejecting the caller's declaration initialization. Source fixes are integrated or assigned to Luna slices and await one combined rebuild. Socket operations and GDB ptrace remain blocked by the sandbox (EPERM); neither is a correctness verdict. Full corpus, final gate, stress/realms, final performance measurement and push remain outstanding.

## CLI36 follow-up and validation friction

CLI36 source `70c7b3dc1db549e806d02847dfc132093e65c330`, SHA-256 `054dfa4ed00d7acb140f016168e3c9bafe6669da7c2faf41de27af02c07b33a2`, built successfully after reconciling current upstream. Node/interpreter six-probe batch: five matched, including both readonly-eval regressions and the module authority probe; function-kind prototype objects still passed through a VM direct-call entry. The subsequent fix rejects them at both VM call entrances, rather than only in the callability predicate.

Native fifteen-probe batch was invalid: every generated runtime failed C compilation because descriptor writes used `kiln_c99_store_has` before its definition. The npm twelve-package selection was stopped after nine compile errors from that same cause; no package execution verdict is accepted. Exact partial evidence is in `../results/targeted-thirty-sixth-invalid-interrupted.json`, preserving the runner's original state plus the supervisor's terminal observation. The shared membership helper now lives beside the property authority before all its consumers. A direct Clang syntax-only check of the retained emitted prelude with that move passed; production-binary validation remains pending.

Network and socket access were restored: Git fetched current main and the Node HTTP upgrade reference printed `200,101,00a5ff`. Main is reconciled through the merge at `70c7b3dc`. GDB remains denied by the host ptrace policy, independently of these permissions. Generated package C is retained under the corpus's own `.kiln/cache`, because the runner executes with the corpus as its working directory; searching the checkout-root cache was the wrong diagnostic path.

Stopping the npm runner with SIGTERM previously bypassed its Ctrl-C evidence/child-cleanup path. It now routes SIGTERM through that existing cancellation path. A real runner cancellation during the Node baseline phase verified interrupted evidence, exit 130, and cleanup of the owned Node child; no native build ran. Exact check log is under `target/npm-compat-evidence/cli36-runner-cancellation/`.

## CLI37 valid targeted evidence and architecture review

CLI37 source `a22260163c59733002fa947e4c53cde3e74d482b`, SHA-256 `ef03cec2e0827d25fd8eacc30184511f1da5bcccf5593bea2c2f8de78b6ed770`: release build and first generated-C callable descriptor preflight passed. The native fifteen-probe batch matched ten, diverged on Reflect.construct, and was invalid on four interpreter references. The Reflect mismatch was two extra native output lines: Symbol and BigInt are valid newTarget constructors even though invoking their own construction throws. The source fix preserves that distinction. The interpreter's Instruction::Call Native fast path still bypassed its function-kind guards, and freezing an ordinary closure left its synthetic prototype descriptor writable; both source fixes await CLI38. Two socket references rejected net.connect options objects; the integrated argument helper now uses ordinary property Gets, and its overload/coercion audit remains open.

The selected npm batch completed eleven of twelve packages: iconv-lite and through2 passed, nine had runtime errors. ws began execution but was interrupted after verifying an idle process with only a listening socket and no client connection. It printed its initial `1 3` output; listening-event delivery remains under investigation. This is neither a passing ws result nor complete twelve-package coverage. The SIGTERM cleanup now preserved state=interrupted, completed=11, selected=12 and exit 130. Exact partial report: `../results/targeted-thirty-seventh-interrupted.json`; raw streams: `conformance/npm-compat/results/raw/20261001T031012Z.2373193`.

Finer Uglify statement diagnostics reach its first prototype assignment, then fail while reading the function binding. Source inspection finds that the native dynamic evaluator treats function declarations as discarded function expressions instead of binding/hoisting declarations. A shared declaration-instantiation fix is required, including calls before declaration; sequential binding would be insufficient. The missing readback marker was itself an expression that threw, and is not evidence of stale source caching.

JWT's symmetric KeyObject and HMAC work in an isolated CLI37 probe, but missing createPublicKey makes jwa's feature detection reject that KeyObject. Correct remediation needs a shared parsed-key import authority exposed by the TS facade; disguising the key as a Buffer or adding a fake import result would hide the API gap. Shared RSA import capability is assigned as a separate architectural slice.

The guarded numeric typed-array read comparison is complete: five alternating measured pairs after one warmup per binary, with both outputs matching Node. Candidate median is 6.045357927 s; control is 9.361239787 s, a 1.548500502 ratio for native process phase wall time including launch/setup, not warmed workload throughput. Control disables only that read branch. Exact source/binary/checksum provenance: `conformance/npm-compat/results/bcrypt-typed-read-comparison-2026-09-30.json`.

No current full 361-package result, final gate, collector stress/realm result, or push exists yet. The goal remains active. Architecture review and new diagnostics are evidence toward completion, not a completion claim.

## CLI38 targeted diagnostic batch

CLI38 source `f3679f414ba53c7e3e85f19970dc3d968addba6f`, SHA-256 `f4b91e38f2c1763d79e0f1f85b18868a018900a991a8d4c696527fca0d60d168`, built successfully. The four affected Node/interpreter oracle probes all matched (function kinds, callable integrity, Reflect.construct, inherited net options getter).

Plain-mode native preflight failed because the upstream global-web bootstrap was now unconditional while structured-clone family selection still pruned its lexical dependency. The same native callable-integrity probe passed in Node mode, whose family selection retains the complete bootstrap. A dependency closure fix is assigned; this distinction does not excuse the plain-mode regression.

The five Node-mode native probes matched Reflect.construct, net options getter, and the four-argument listen callback plus bound listening listener. HTTP upgrade was invalid because the interpreter's accepted HTTP socket facade omitted write/end/destroy methods. Binary TCP diverged because native TCP reads always returned strings even without setEncoding. Both underlying source fixes are integrated and await the next validation batch.

The four-package diagnostic batch completed: Babel, Prettier and deep-equal had runtime errors; ws timed out after 60 seconds. Selected=4, deferred=357, discovered=361; no passes. Exact report: `../results/targeted-thirty-eighth-four.json`. The named builtin-refusal diagnostic identifies SharedArrayBuffer as deep-equal's computed own-key authority gap. Babel's error changed to an undefined callee in `_traverse` at instruction 45; previous `crawl` assumptions no longer describe this run. Raw streams remain under `conformance/npm-compat/results/raw/20261001T033954Z.2409815`.

The dynamic evaluator's function-body declaration hoisting and nullish-write correction are integrated after CLI38, as are custom Map/Set/Promise own-property checks and further net argument validation. None is claimed validated by the CLI38 binary.

## Source batch after CLI38: architecture and friction audit

The explicit one-shot ws listening diagnostic proves that both the outer WS server and inner HTTP server receive listening events. The earlier idle-process observation did not establish a listening-delivery defect. The handshake trace instead proves that ws rejects the accepted native socket at `completeUpgrade` because its `readable` and `writable` fields are missing; the actual HTTP Upgrade request arrives successfully. Source fixes project duplex state from transport state and unify interpreter HTTP/upgraded TCP socket identity. TCP data now preserves binary Buffer chunks without setEncoding. These changes still require fresh native/package validation.

Native transport errors were also strings instead of Error objects. The shared completion/event path now constructs Error values; a refused-connection fixture checks Error identity and message presence. ClientRequest public socket assignment and its deferred socket event are being audited separately.

The unconditional web bootstrap now retains its structuredClone/ArrayBuffer and Math/Number dependencies. ArrayBuffer/SharedArrayBuffer constructor reflection now uses intrinsic prototype identity and the ordinary installed species accessor. Review corrected the species accessor's enumerability before validation.

Parsed RSA key import/export is implemented through one shared Rust core and both engines' private adapters. Review caught private method-ID collisions and omitted generated header definitions; both are corrected. Dependency resolution added fifteen packages without upgrading existing package versions. Native key lifetime release is still being tightened; imported-key capability does not imply RSA signing support or completed JWT compatibility.

Function diagnostic headers preserve existing emitter contracts and include inferred names. The Babel failure's named frame does not match a Call at PC45 in the retained compiled `_traverse` body; instruction attribution (including dynamic execution) remains under investigation. This discrepancy alone does not establish stale artifacts or cache corruption.

The next batch is one combined release build, bootstrap/native preflights, and targeted package diagnostics. Full 361-package execution, final routine gate, affected collector/realm checks, and normal push to main remain required. No new passing count is inferred from source review.


## CLI39 and CLI40 follow-up

CLI39 built at `aa677269`, but plain native preflight failed C compilation: early HTTP client attachment used `kiln_c99_put` before its declaration. No package verdict is inferred. The declaration is corrected. A later failed Rust build exposed socket end-state access on the driver rather than its existing duplex-state API; this is corrected too.

CLI40 source `de2de4d5a8dc16e4f65cb3755537e57c08ed75c8`, binary SHA-256 `c953f0d34d897c2791ca4a7a41ccec900da6dbef31fec6447aab61aaa877b78a`: release build and plain native callable-integrity preflight passed. Native nine-probe batch: 3 matched, 2 diverged, 4 invalid. Connect-overload interpreter exceeded a minute and was supervised/terminated. Raw log: `target/npm-compat-evidence/cli40-native-nine.log`; completed result files were preserved before termination.

ArrayBuffer reflection also fails on Node due to the wrong fixture expectation; it is corrected to `@@Symbol.species`. Dynamic async functions returned scalars instead of Promises because they lacked the generator-backed flag. Native RSA metadata used raw C strings instead of canonical runtime string headers. Both source corrections are integrated after CLI40 and await validation. The native runner truncated divergent outputs to five lines, hiding later failures; failing pairs now retain complete output.

CLI40 npm completed all nine selected packages: JWT passed, seven runtime errors, ws timeout at 90 seconds. Coverage selected=9, deferred=352, discovered=361. Report: `../results/targeted-fortieth-nine.json`; raw streams/binaries: `conformance/npm-compat/results/raw/20261001T050754Z.2460126`. Deep-equal names Boolean constructor reflection; Prettier reveals internal CELL reaching own-property inspection. Babel, Arktype, Effect and Svelte still fail. Full 361 validation, final gate, affected GC/realm checks and push remain required.

## CLI41/42 integration and compile diagnostic

CLI41 production source `e5c6d130`, binary SHA256 `1ef74ca32888588ba936cc575705f886a4d89c98f5d3bc47823b9800c0c6fdca`, completed its Rust release build in 2m45s. The seven-package batch completed with seven C compile errors and no runtime verdicts; selected=7, deferred=354, discovered=361. Raw evidence: `conformance/npm-compat/results/raw/20261001T055801Z.2510056`; retained report: `../results/targeted-forty-first-seven.json`. The common error is early socket helpers in foundation.c using `kiln_c99_method` and `kiln_c99_heap_store_pointer` before declaration. Root `85bd7219` moves those helpers to node.c after their dependencies instead of adding more cross-domain forward declarations. The property preflight hit its 90-second supervision limit while packaging the changed provider; this is incomplete validation, not a property verdict. Future native probes use the npm TMPDIR to share provider packaging artifacts.

CLI42 source `90522f5c`, binary SHA256 `1724896873f6bba967a4df7974b39b0173ced00d799fc296e6feebc050efb0e5`, adds opt-in module demand tracing and the missing interpreter socket EventEmitter surface. It built successfully in 32.38s but predates the C order repair. Use it only for emission-only Svelte demand diagnosis.

Further integrated source fixes await CLI43: computed class Symbol keys and inherited constructor membership (`1b6e9021`), bound-callable getter receiver (`beb02acc`), namespace Object.assign live export reads (`e5c6d130`), correct client helper placement (`85bd7219`), and token-aware complete dynamic loop statement capture (`b677d054`). Final whole-corpus verification, gate, affected GC/realm/HTTP checks, and push remain pending.

## CLI43 targeted execution evidence

CLI43 production source `b677d054` (documentation-only `8f1df14c` at copy time), SHA256 `bde634e947bd47034c7041d5c6630f071a27e8ea6609955a103c657fd2f468b2`, built successfully in 20.74s. The small native preflight compiled and reported 2 matched (bound-callable getter and dynamic loop body), 1 diverged (the computed Symbol/class fixture now agrees except interpreter erroneously reports a callable method after setting the function's prototype to null). Root `1865e449` changes interpreter callable membership to consult its actual prototype. Native stress/realm checks remain pending.

The seven-package batch completed: Prettier, Effect and JWT passed; four runtime errors; selected=7, deferred=354, discovered=361. Raw evidence: `conformance/npm-compat/results/raw/20261001T060500Z.2519532`; report: `../results/targeted-forty-third-seven.json`. Uglify now gives a precise dynamic Function byte-23 parser error near `< pos;`; deep-equal advances to missing native method length authority; Arktype advances from getter receiver failure to dynamic `Expected '}'`; Babel remains undefined-call failure. These are targeted results, not a whole-corpus verdict.

The npm runner builds the default `native-dev` profile (NativeProfile::default is Dev). Svelte demand diagnostics must match that profile; a native-release explanation has a different slot numbering and cannot explain the retained native-dev artifact. Opt-in tracing is being used to check native import-meta reparse demand remapping.


## CLI44 focused validation (2026-10-01)

Production source `48fddba2`, binary SHA256 `4a057ca4d68d940fc458579879c7fef48e5632466f9864ecaca51ccd8badd517`. The six-package batch completed with one pass (deep-equal), four runtime errors and one runtime timeout; selected=6, deferred=355, discovered=361. Complete evidence is `conformance/npm-compat/results/raw/20261001T062137Z.2534858`, retained report `../results/targeted-forty-fourth-six.json`. Uglify now reaches an undetailed backend refusal; Arktype exposes missing dynamic object-method syntax; Svelte reaches missing Math.f16round after its demanded tsPlugin export is correctly remapped from slot 74 to 59. Babel returns inconsistent native status values on repeat execution, so its cause is still unresolved. ws upgrade reaches Socket.cork/uncork calls that are absent; its callback failure was silently parked by the native HTTP driver. Source fixes for these identified causes require a combined refresh.

The ten-probe native batch reports seven matches, no explicit refusals and three divergences. RSA key objects, class symbol inheritance, bound getter receivers, dynamic for-in and bitwise parsing, Date.getTime arity and ArrayBuffer reflection match. BigInt static methods, interpreter computed namespace reads, and dynamic var declaration capture still diverge on this pin. Subsequent source changes address them, but these results do not validate those later changes. Complete log: `target/npm-compat-evidence/cli44-native-ten.log`. Final whole corpus, gate, stress checks and push remain outstanding.


## HTTP transport source and focused checks after CLI44

`547a6958` disables Hyper HTTP/1 pipeline flushing before manual upgrade handoff. In the pinned Hyper source, pipeline flushing returns ready without writing when read_buf is nonempty; raw upgraded bytes remain in that buffer, and into_parts retains the reader prefix but discards pending HTTP writes. A duplex transport regression now sends an ordinary request, an upgrade and raw bytes in one write and asserts ordered 200/101 responses and exact raw bytes. Focused `mbx test -p kiln-runtime pipelined_upgrade_flushes_http_responses_before_raw_transport_handoff` passed 1/1; full log `target/npm-compat-evidence/runtime-upgrade-unit.log`. HTTP throughput comparison and cross-engine fixture validation remain pending.

The native callback failure propagation unit passed 1/1 after correcting its mock event from UpgradeResponse (which requires transport adoption) to ResponseHead. It exercises the same generated-language callback path, verifies that the first failure reaches the parked drive, and verifies that a second callback is suppressed. Initial setup failure and corrected passing log are both retained (`provider-http-failure-unit.log`, `provider-http-failure-unit-corrected.log`). No broader verdict follows from these two checks.

CLI44 interpreter execution of node-net-connect-overloads remains unresolved: the owned process hit the 25-second supervision limit with no stderr. Its stdout/stderr are retained as `cli44-net-overloads-interp.*`. A live stack attachment was attempted after the process had terminated and yielded no stack; this is not evidence of the hang's cause.


## Queued resource cancellation after CLI44

The instrumented CLI44 net-overload probe records successful port validation, listening and the connect callback before its timeout. Thus the current hang does not prove broken overload callback dispatch. Progress markers are retained in `target/npm-compat-evidence/net-overloads-progress.log`; probe stdout/stderr are `cli44-net-progress.*`. A live stack attachment was denied by ptrace and yielded no stack.

Root `51604ce1` repairs host ownership when a request is cancelled after its completion has been promoted but before delivery. Promotion has already installed a listener or accepted/connected socket in the host's resource registry; cancellation previously removed only the ready completion. It now closes that undelivered resource as well. The focused cancellation batch passes 8 tests, including listener liveness retirement and connected/accepted peer EOF, in `runtime-ready-cancellation-units.log`. This establishes the ownership repair; a fresh interpreter/native overload probe is still required to prove whether it resolves the observed shutdown hang.


## Historical package coverage audit before the next refresh

Combining the latest recorded result for each inventory package identifies six outstanding non-passes: @babel/core, arktype, handlebars, svelte, uglify-js and ws. Handlebars was last checked on CLI37 and fails its each helper on an undefined `options.fn` callback after the first template succeeds; it must be included in the next targeted refresh. Other packages have passing historical evidence, but those results span different source pins and do not prove all 361 pass on the current source. Final whole-corpus execution is still required.

Uglify's CLI44 refusal is traced to `Object.create(base && base.prototype)` in ast.js. The captured parent environment lacks AST_Node, so DEFNODE's defaulting assignment reads undefined. That pin predates `80f34972`'s var pre-instantiation, which addresses the shared capture cause. No separate package-specific patch is justified; the next pin must validate it. Durable source and scope traces are `uglify-cli44-refusal-source.log` and `uglify-cli44-dynamic-scope.log` under the evidence directory.


## Integrated source before CLI45

`ec295add` fixes Babel's nondeterministic status: the generated namespace wrapper left status uninitialized when extending an existing namespace with zero exports. The new C fixture exercises that valid no-op path. Review of other emitted wrapper status declarations found no additional uninitialized path.

`20c6305f` keeps ordinary static Math uses out of the Float16 augmentation and its typed-array dependency, while retaining f16round for direct demand and Math property reflection. `6a09284b` extends dynamic var declaration instantiation across control-flow blocks without collecting vars from nested callable bodies, gives concise methods names and nonconstructible method kind, and replaces source-length-sized scan buffers with depth-sized growth. Review corrected single-parameter arrow argument placement before integration; `4f9ee6cd` makes the construction regression distinguish successful construction from a TypeError. These source changes still require the combined binary refresh and focused probes.

Socket public cork/uncork methods are being implemented in TypeScript over private buffer operations. Review requires stream state transition before write callbacks, callback-driven destruction guards, end overload normalization, immutable per-realm registration, and per-write upgrade completion fences. The public-C-method intermediate is not yet integrated; the coherent TS facade and runtime event seam must be reviewed together before the release build.

## Combined socket and dynamic-loop source integration

The final TypeScript facade from the socket slice is now integrated through `18f2a4bf`; its intermediate public C methods are removed in the final source. Upgrade append receipts and completion events are integrated through `d1dea8ed`. Each append has its own fence, and host prefix writes flush the corresponding boundary before recording success. Cancellation preserves already-completed fences and releases pending waiters with an error. Source review found and corrected a missing helper and a mutable map borrow crossing language callbacks before any build.

The socket adapter remains unfinished: review identified pending write callbacks lost on connection failure or destroy, interpreter own methods shadowing the facade, end callbacks registered before argument validation, and missing cork-buffer backpressure accounting. The owning agent is fixing these together with the native/interpreter fence adapters. No source-only check establishes runtime correctness yet.

`6c99e695` adds dynamic while execution, including return and labeled loop control, rather than dropping a valid var-capture fixture. Follow-up review identified stacked labels on loops as a remaining shared control-flow defect; this is being repaired before the combined build. The next validation batch must include the six outstanding packages, then refresh all 361 inventory rows after targeted failures are resolved. Final gate, affected stress/realm checks, HTTP close contract and throughput comparison, remote reconciliation and main push remain required.

## Late helper validation before CLI45

The completed timeout and benchmark helpers were checked together once: 6 native-timeout tests, 5 server-comparison tests and 6 Node client tests passed. Logs are `target/npm-compat-evidence/native-timeout-tests.log`, `server-compare-tests.log` and `server-drive-tests.log`. Timeout coverage includes descendant cleanup, escaped pipe owners, interruption, signal exit status and batch continuation; benchmark coverage validates fixed-length/chunked replies and rejects incorrect bodies. These results do not establish package compatibility or Rust/native correctness.

Socket cleanup source through `40a32a76` clears queued state and separates end callbacks from finish listeners. A focused installed-Node v26.8.1 oracle found the remaining callback contract: end callbacks have a receiver other than the socket and exactly one argument, explicit null on success or the structured error on failure. A stronger identity oracle corrected the initial inference of undefined; Node uses an internal callback-list receiver, which is not a documented receiver contract. Failed connect reports ECONNREFUSED; explicit destroy reports ERR_STREAM_DESTROYED. The owning slice is correcting receiver/argument handling and sharing structured completion errors across callbacks and the socket error event before the combined release build.

Socket callback repairs are integrated through `26bb35fb`. Source review corrected a missing private C forward declaration and literal byte lengths before building. The combined source still awaits native and package validation.

## CLI45 native preflight

CLI45 source `d1482f0e`, SHA256 `f15c3634b94adf36f761fc0058e33295ce20a9d937b3e34d1d00fafd3db94c0f`, built successfully after correcting the configured-hasher map constructor. Its first native preflight failed C compilation: fifteen new socket callback array accesses incorrectly applied boxed `KILN_VAL_PTR` to boundary `kiln_value` structs. `9176080c` switches these to the established `kiln_c99_array_of` accessor. The complete compile diagnostics remain in `target/npm-compat-evidence/cli45-native-preflight.log`. No package verdict is claimed for CLI45.

`5be22f42` adds a regression observing actual prefix-writer polls after cancellation, rather than merely inspecting completion state. The test lives inside the HTTP engine module to retain private setup APIs. The rebuilt combined source will receive the focused native/package and runtime checks together.

## CLI46 focused validation

CLI46 production source `043e3f239d10234ee1a4912fb3128e87d37d66c7`, SHA256 `599b5dae2c7a788d11d96ca9c61b9e91b94370dec65c27b0cc3094e805ce05bd`, built successfully. Native provider preflight matched. All 16 HTTP engine unit tests passed, including ordered upgrade fences, abort publication and pre-cancelled writer polling; the native realm callback-failure regression also passed.

The six-package report completed with Babel and Svelte passing, three runtime errors (Uglify, Arktype and Handlebars), and ws failing to resolve the new net dependency from its virtual HTTP builtin. Selected=6, deferred=355, discovered=361. Retained report: `../results/targeted-forty-sixth-six.json`; complete raw evidence: `conformance/npm-compat/results/raw/cli46-targeted-six-corrected`. The initial relative evidence-directory invocation produced false missing-binary compile-error labels; `6288bc85` resolves artifact paths before command execution, and that initial run was interrupted rather than accepted as compatibility evidence.

The 18-probe native batch completed with 5 matches, 2 divergences and 11 invalid probes, including two bounded deadlines. Full log: `target/npm-compat-evidence/cli46-native-eighteen.log`. Dynamic arrows and quoted method keys diverge; stacked labels fail interpreter parsing. The socket TypeScript facade is not consistently installed in the interpreter, HTTP virtual imports fail resolution, and close/error ownership still needs correction. These failures remain assigned to the coordinated slices.

A focused Handlebars diagnostic established that ordinary helper callbacks work and the runtime container's program/noop values are functions, but the dynamic template options object has corrupt quoted property names. The shared dynamic object parser retains a decoded string token pointer after lexer advancement frees it. This explains both missing fn/inverse and the quoted-method divergence; the fix must preserve owned keys across value evaluation and allocation.

The namespace wrapper unit fixture failed linking because its fake provider omitted eight current prelude exports. `5cfdec11` adds explicit unsupported stubs for these unreachable seams; the corrected focused fixture passed (1 passed, 0 failed), with full log `target/npm-compat-evidence/cli46-namespace-wrapper-test-corrected.log`. No test verdict is inferred from a build or historical package pass.


## CLI47 focused validation

CLI47 production source `575627eb24e79cc7172c311bb7d1ec89ee0853d9`, SHA256 `98c5a754551c61c5f25d1c4ff072d53c7dd065ed820bde8438aeb93287ee558d`, built successfully; native provider preflight matched. The four-package refresh completed with Handlebars passing and Uglify, Arktype and ws reporting runtime errors. Selected=4, deferred=357, discovered=361. Report: `../results/targeted-forty-seventh-four.json`; full raw evidence: `conformance/npm-compat/results/raw/cli47-targeted-four`. Babel and Svelte's prior CLI46 passes remain historical. No full current-tree result is claimed.

The 20-probe batch completed with 8 matches, 2 divergences and 10 invalid probes, including three bounded deadlines. Full log: `target/npm-compat-evidence/cli47-native-twenty.log`. Failed-connect close ownership now matches. Interpreter stacked labels pass, but native labeled-block break and Unicode/empty quoted keys remain divergent. Interpreter private cork/pause/resume capabilities are still inaccessible; the package import fixes alone do not prove the facade works. The listener-identity probe was narrowed after this pin to use supported on(), preserving its removal-by-original-identity and socket facade assertions.

Bounded diagnostics identify Uglify's next cause as the dynamic evaluator's missing switch grammar (`mozilla-ast.js` map function), and ws's undefined constructor as Buffer[Symbol.species] (`Receiver.consume`). These require shared language/builtin repairs. A separate interpreter checkpoint diagnostic reaches listening, accepted, connected, successful write callback and peer data, then hangs after callback-driven destroy. Source inspection finds the destroyed-socket guard also drops the later authoritative close completion; the pending source repair preserves close while suppressing stale selected work and allowing common stream retirement. These repairs are not yet validated.


## Post-CLI47 source integration and cancellation diagnosis

Source through `eea9a129` integrates dynamic switch parsing, labeled-block lexer cleanup and Unicode quoted-key conversion, Buffer FastBuffer species with Uint8Array construction semantics and Node constructor metadata, and an explicit interpreter private net capability object. These changes remain unvalidated until the next consolidated binary refresh. Review corrected case-inventory validation order and FastBuffer undefined/string/fractional/typed-array inputs instead of limiting its implementation to ws's observed arguments.

`b7ca7039`, `f35739b6` and `12f00cd9` preserve close completion after callback-driven destruction and drain raw sockets' writable half after EOF. `eea9a129` repairs the native EOF path, which previously force-closed after end listeners and could discard their final writes. Its focused regression requires the reply bytes, successful write callback and both socket closes. Public allowHalfOpen is respected; final option propagation and live behavior still require validation.

A bounded CLI47 native diagnostic reaches connect callback, client end submission, server close and client finish, but never the accepted-socket callback or client EOF/close. It times out after 12 seconds. Evidence: `target/npm-compat-evidence/cli47-net-overloads-progress2.run.log` and `net-overloads-progress.log`. The provider had already moved the accept completion into its language queue when the connect listener cancelled it; cancellation suppressed the callback but left its newly created transport alive. `b69ac59f` records completion-owned resources until callback delivery and retires them once on cancellation. ResourceState queries do not own the resource they describe. A focused provider regression with real transports is being added by the owning Luna slice; no build/test result is claimed yet.


## Provider regression and shared read/event authority

The actual queued-resource regression passed: 1 passed, 0 failed, on source `1c0fb74c` (provider implementation `b69ac59f`). Full log: `target/npm-compat-evidence/provider-queued-cancellation-test-runtime-corrected.log`. It verifies callback suppression and disposer counts for a queued accept and borrowed state query, accepted-resource retirement, peer EOF and listener availability. The initial fixture failed to compile because its TCP port was u32; after correction it aborted because executor polling was nested inside an active realm entry. `072cee54` and `1c0fb74c` correct these fixture issues. Later changes only remove unused mut warnings. These failed attempts are retained; they are not production runtime verdicts.

`5e54f0b5` and `8713e214` repair Arktype's exact shared cause: a bound callable's constructor read skipped its custom prototype and returned global Function. `bindReference` consequently constructed a Function from attachments, with source `[object Object]` and no impl. Bound reads now consult own properties, own name/length and their actual prototype before synthetic callable shortcuts. Its focused fixture also covers own call shadowing, constructor overrides and an explicit null prototype.

`f4ead5e0` allows optional interpreter HTTP request listeners and dispatches request events through the server emitter, preserving supplied listener identity and receiver. Native HTTP event dispatch must receive the matching repair before the consolidated pin. Review also identified active server wrapper liveness: the host retains its state while only weakly retaining the object containing listeners. The owning VM/ModuleRealm must root that object during listen/drain and release it on terminal close or teardown. The socket option slice is projecting Node's distinct allowHalfOpen defaults for net and HTTP sockets. These source slices remain in progress.

`a73394a1` stops raw server close from forcibly destroying accepted sockets and defers its close event until they drain. `97031ff0` adds a close-from-connection-listener regression. `c4c93c0d` keeps a switch label from becoming an inner loop's label. The next package refresh and affected native batch must validate these integrated changes together before the 361-row final run.

## Consolidated source after CLI47

The queued-accept cancellation regression now passes with real transports (1 passed, 0 failed); evidence is `target/npm-compat-evidence/provider-queued-cancellation-test-runtime-corrected.log`. The test separately proves cancellation disposes the accepted resource exactly once while preserving borrowed ResourceState ownership. The fixture executor correction is test-only.

Shared dynamic-switch and quoted-key handling, Buffer species construction, bound-callable property lookup, socket EOF/write draining, raw server-close draining, optional HTTP listener dispatch, and half-open policy fixes are integrated. Active interpreter server wrappers are rooted by the owning VM/module realm until terminal close without adding a state/object Rc cycle. The added unreferenced-server regression covers listener receiver identity. These source fixes await one consolidated CLI48 build and targeted package/native validation; no new package passing count is inferred.

## CLI48 targeted evidence

CLI48 source `c93a2c45592009a2dfea20e07b74f50a279371d6`, SHA256 `b79ebec828b21aab47b67ec8a0fbf2300563246c3af8a742407cd2979467704d`, release build passed in 2m26s. Plain callable-integrity and Node HTTP overload native preflights both matched. The three-package refresh completed: Arktype passes; Uglify reports a missing dynamic `instanceof` parse branch, and ws now exchanges both messages before a socketOnClose property failure. Selected=3, deferred=358, discovered=361. Report: `../results/targeted-forty-eighth-three.json`; raw evidence: `conformance/npm-compat/results/raw/cli48-targeted-three`.

The affected 23-probe batch completed with 12 matches, 4 divergences and 7 invalid probes, including 3 bounded deadlines. Evidence: `target/npm-compat-evidence/cli48-native-affected.log`. Further source review identified switch lexical TDZ and custom Buffer species authority gaps. The owning slices are repairing those, dynamic instanceof, bound callable prototype lookup, queued write callback ownership, HTTP response event dispatch and client IncomingMessage socket projection. These are unfinished, not waived failures.

The original EOF regression passes under interpreter CLI48, but native exceeds a bounded 20-second deadline. A native server-close diagnostic receives the request, writes its reply, closes the accepted socket and server, then exits without ever delivering client data or close. The raw TCP connect completion skips starting its read lane when a connect listener ends the writer. The root source fix starts reading independently before handling pending writes/shutdown and avoids a second shutdown while a write or shutdown is already in flight. Evidence: `target/npm-compat-evidence/cli48-eof-original-interp.log`, `cli48-eof-original-native.log`, `cli48-server-close-progress-native.log`. Validation is pending the next consolidated source batch.

## Post-CLI48 shared source repairs

The HTTP client convenience callback is registered on the same response emitter as later listeners, rather than being called through a separate channel; redundant callback roots were removed. Client IncomingMessage now exposes its socket/connection while active. The response regression checks listener order, receiver identity, shared response identity and removal of the optional callback. Node passes that fixture after moving socket identity checks before Node detaches its socket at end.

Interpreter socket writes now retain FIFO callback groups aligned with the duplex driver write queue. Destroy settles accepted-write callbacks before deferred/end callbacks and close; successful write callbacks use one null error argument, confirmed by three real Node TCP cases. Bound callable property reads follow actual prototypes after own/identity properties. These changes remain source-only until the next consolidated CLI. Native callback ownership, buffered socket reads, and dynamic/Buffer source slices are in progress.


## Pending-only cancellation and reviewed declaration inventory

The submitted-write cancellation path cannot infer success from submission. Real Node TCP oracles distinguish an already accepted small write (null callback error) from a 16 MiB backpressured write (ECANCELED); deferred writes/end callbacks use ERR_STREAM_DESTROYED. Source `4138964c`/`a468123a` adds a shared pending-only cancellation result that preserves ready host/provider completions and cancels only unfinished work. This is a real additive ABI change (provider ABI version 70, 175 exports). Source `f691b34b` polls a raw TCP write once against actual OS I/O, retaining the same future and write lock when partial progress remains, so immediate destruction cannot turn a real completed write into cancellation. Real-resource regressions are added but not yet run. Upgraded writes retain their existing fence authority.

Source through `11c3d007` integrates the reviewed dynamic instanceof operator, switch lexical TDZ and CaseBlock var conflicts, unresolved identifier ReferenceError, shared TS Buffer call/new allocation facade, and custom species behavior. Switch var conflicts now inventory CaseBlock declarations once and share callable boundary tracking with function hoisting; outer function vars and nested callable vars do not incorrectly reject lexical shadowing. Node oracle checks passed; no new Kiln verdict is claimed. Native write FIFO/cancellation and readable-state source remain under review before one consolidated refresh.


The pending-only cancellation runtime batch passes on source `780bd97c`: 2 passed, 0 failed (221 filtered). It proves ready ResourceState preservation, unfinished accept cancellation, and a synchronous TCP write whose actual peer byte and successful completion survive immediate cancellation. Full evidence: `target/npm-compat-evidence/pending-only-cancellation-tests-corrected.log`. The initial regression build failed on the fixture's TcpConnect port type (u16, unlike TcpListen's u32); the correction and failed log are retained. This is focused host-runtime evidence, not validation of the still-pending native/interpreter socket consumers or the full npm inventory.


Source through `c35cd6d0` integrates native serialized raw TCP write submission, GC-traced FIFO/callback batches, preconnect ordering, active-write ownership, backpressure and amortized queue/byte compaction. Pending-only cancellation consumers now preserve real ready results, report ECANCELED only for unfinished submitted writes, and settle never-submitted/end callbacks separately. Source review corrected unrooted temporary callback arrays and a fixed-size compaction threshold that would still repeatedly copy a large queue. Interpreter deferred callback values remain owned by their Rc socket state. These consumers await consolidated native validation.

`7d2d0e96` repairs legacy numeric Buffer call/new overloads through shared existing allocation authority: additional numeric/nullish arguments are ignored and string encoding arguments are rejected with ERR_INVALID_ARG_TYPE. Native callable/constructor dispatch share the same helper; interpreter uses the existing public TS alloc facade. The added Node oracle assertions pass. Buffered socket read/EOF/refill source is still under review, and HTTP upgrade write-fence diagnosis remains outstanding.


The exact 16 MiB regression's Node v26.8.1 oracle exposed another propagation edge during source audit: its waiting end callback inherits ECANCELED from the failed active write, with one error argument, before close. An end callback after an already-successful active write and explicit destroy is a distinct case. The owning execution slice is correcting the fixture and end-callback propagation; earlier generic destroy-error wording does not establish this contract. No Kiln validation of that consumer is claimed.


The extended real-transport provider regression passes on `85820b86`: 1 passed, 0 failed (59 filtered), build/test phase 6.47 seconds. It now checks the new pending-only API preserves already-queued accept and borrowed ResourceState completions/ownership before ordinary cancellation still retires the accepted resource exactly once. Full evidence: `target/npm-compat-evidence/provider-pending-only-queued-cancellation-test.log`. This supplements the two focused host-runtime checks; socket consumer parity and the current npm inventory remain pending.


## Consolidated CLI49 source and late validation

Source `adf3656f` integrates the reviewed readable-state capability, one existing transport/read buffer per engine, paused read-demand refill, decimal read-size normalization, and deferred EOF/end publication. It preserves Hyper's exclusive HTTP reader until upgrade transfer, and carries readable-listener exceptions through the native completion seam. Source review also corrected a merge-time write-completion early return that could drop independent writer shutdown/close decisions. Node passes the focused readable fixture, including fractional decimal parsing and bounded/unbounded reads before data or EOF.

The real Node response-end reentry oracle shows a detached old response socket, no synchronous socket on the nested request, and a second connection. Therefore the Agent retention query is pure, end callbacks run before release publishes a reusable connection or reserves a queued request, cleanup removes only the ending request, and release respects callback-driven close/errors. The private provider query is additive ABI version 71, 176 exports. `http-client-end-reentry.cjs` verifies this observed ownership sequence under Node. No Kiln verdict is inferred until the consolidated build/parity batch.

The CLI48 upgrade trace reaches UpgradeReady, all three original write-fence completions, and normal HTTP connection future completion; its bounded timeout is downstream in raw socket lifecycle. No speculative HTTP fence implementation change was made. Evidence: `target/npm-compat-evidence/cli48-upgrade-original-httptrace.log`. The new raw socket ownership/read repairs must validate the original regression. CLI49 release build is now running once for the integrated source; full npm, matched HTTP performance, gate/stress and main push remain outstanding.


CLI49 release on `8f61c794` passed after correcting an interpreter completion socket-state type mismatch. Its native preflight failed before execution: the shared dynamic callable scanner lacked a closing parenthesis, and the existing instanceof helper needed a forward declaration before dynamic.c. `f81999a3` corrects both using the existing helper and declaration authority; CLI50 is the next consolidated release pin. No CLI49 package/native passes are claimed. Evidence: `target/npm-compat-evidence/cli49-build-corrected.log` and `cli49-native-preflight.log`.


## CLI50 measured targeted result

The corrected source pin `f81999a3` passed its release build and both native provider preflights (plain callable integrity and Node HTTP createServer overloads, one matched each). Package coverage is selected 3 / deferred 358 / discovered 361: ws and Arktype pass; Uglify reaches a new dynamic update-expression SyntaxError (`Expected member reference after update operator`, near `i >= 0`). Exact source, binary digest and phase timings are retained in `results/targeted-fiftieth-three.json`.

The 29 affected native probes report 18 matched, 1 refused, 3 diverged, 7 invalid, including 2 deadline expirations. Remaining measured issues are dynamic switch refusal, Buffer constructor/species, callable null prototype, cancellation completion losing its stream, large-write callback/close ordering, missing HTTP response removeListener/native socket resume, HTTP end reentry connection ownership, readable-state timeout and upgrade lifecycle timeout. The three owning Luna slices are correcting these findings; no complete npm or final gate/performance claim is made. Full evidence: `target/npm-compat-evidence/cli50-native-affected.log`.


Source `cd1a79cc` integrates the cancellation/driver identity repair: ready completions still resolve their stream; confirmed cancellation explicitly retires driver bookkeeping. Deferred write/end callbacks precede the scheduled close event, and native ready-write destruction waits for the active callback before discarding queued writes. `7dc321a1` keeps the new deferred event in its owning realm. Source `30c09726` shares dynamic prefix/postfix identifier resolution and existing checked ToNumeric/increment/decrement authority, including Node-oracle coverage of string/BigInt/object conversion and unresolved/restricted writes. Both repairs remain source-only pending the next combined validation pin.

The bounded CLI50 interpreter HTTP trace still dispatches all three upgrade write completions and ends the HTTP connection future normally before hanging downstream (`target/npm-compat-evidence/cli50-upgrade-interpreter-httptrace.log`, timeout 25 seconds). This does not justify an upgrade-fence notification change. HTTP server accept paths suppress the standard connection event, independently explaining the new end-reentry fixture connection-count failure; the module slice is correcting that emitter path with the other measured lifecycle failures. Native timeout diagnostics now name interpreter versus native phase, and npm JSON evidence explicitly records discovered inventory count.


Source through `5b9cc7a2` integrates the reviewed CLI50 follow-ups: bound lookup stops at a null prototype, the existing Buffer facade is classified as constructible, and update references walk lexical/with environments in order while avoiding a second getter or numeric coercion. Node oracle fixtures cover the accessor and lexical-shadowing cases. Source `79c86f62` exposes existing private net capabilities to the TS socket facade, completes request/socket EventEmitter surfaces, restores HTTP connection events, and accounts retiring Agent slots until their real close. `04698b1b` attaches accepted HTTP sockets before publishing the connection event; its Node fixture proves a listener can destroy the owned socket before request parsing. `5b9cc7a2` reads EOF demand from NetSocketState rather than inventing a generic ReadableState field. `6758f605` retains private capability temporaries through publication. Formatting and diff checks pass; engine verdicts still await the combined CLI refresh.

The remaining switch refusal was isolated to a CaseBlock function declaration excluded by the old top-level function inventory. Correct CaseBlock binding instantiation, ASI boundaries and sloppy/strict declaration scope are under implementation, rather than removing the refusal guard or dropping the valid fixture. No additional release build or full corpus run has been spent while that source slice is still changing.


## Published checkpoint and continued integration

At the user's request to push ready work while continuing, the committed implementation/evidence checkpoint was published to `origin/fix/npm-compat-20261001` and its `34f66e18` remote SHA verified. The latest main (`28662738`, seven GUI commits after `05947f06`) was then merged without conflicts as `72575fc3`; the checkpoint branch was updated by normal push. Main has not received the npm changes yet because the required final gate/current engine validation remains pending. The explicit main-push authorization persists, and validated batches can land while the larger npm goal continues.

`34f66e18` also limits interpreter HTTP server close to runtime-http-owned streams, preserving upgraded raw StreamDriver close authority. That fixes a demonstrated ownership mismatch, but no claim is made that it resolves the CLI50 upgrade timeout. Correct switch function instantiation is still under review for ASI and Annex B eligibility, hoisting and declaration-reached assignment.


## CLI51 checkpoint and preflight

The remote checkpoint advanced through `d26abba3` (verified), integrating CaseBlock declaration instantiation and the native net capability key-length correction. Main remains pending current validation. CLI51 release built successfully in 128 seconds; exact source `d26abba3bb043b86a520cddc5d6f717f5359af5b`, binary SHA-256 `a66b96ec32822dfcaf17ace8b772b2fab86433689fdb04a90faeffd5511b3b79`. Its callable native preflight matched 1 probe with no refusal/divergence; the Node provider preflight is warming its canonical cache. Logs and immutable pin are under `target/npm-compat-evidence/cli51-*`.

Independent review found two further Annex B issues before final validation: the function-entry switch scanner advances past its closing parenthesis, and later surrounding lexical declarations are absent during eligibility checks. The owning Luna slice is correcting lexical declaration instantiation/TDZ and hoisting; CLI51 must not be claimed as the final switch fix. Raw upgrade review found no evidence of an unresolved write-fence notification; all fences and the HTTP future complete in the retained trace. The original lifecycle regression still requires execution against current source.


Both CLI51 preflights matched 1 probe each (plain callable and Node HTTP). The UTC package slice completed selected 3 / deferred 358 / discovered 361: ws and Arktype pass again; Uglify now advances beyond the update-operator failure and reports `Unexpected token: name «r», expected: punc «;»` from its own parser. This is not a pass or a throughput measurement. Exact pin, digest, coverage and phase timings are committed in `results/targeted-fifty-first-three.json`; the execution slice is diagnosing that new failure. The 25-probe socket/Buffer/callable batch is running on the same immutable pin, omitting the changing dynamic Function fixtures and the already-passed HTTP overload preflight.


## Main publication and CLI51 socket verdict

The user clarified that publication must go directly to main while work continues. The normal main push succeeded at `54e5cb47`, preserving new GUI/Taffy main commit `84933931`. This explicit instruction supersedes waiting for the final gate before publication; validation remains incomplete and the goal stays active. Subsequent reviewed lexical-instantiation source (`c92e32b7`, `a9743f60`) corrects switch parenthesis capture, function/block TDZ cells, scope identity instead of brace-depth collision, and comma-separated lexical declarations. Node oracles and diff checks pass; no Kiln verdict is inferred for this follow-up.

CLI51's 25 affected non-dynamic probes completed: 17 matched, 4 diverged, 4 invalid, including one interpreter upgrade deadline. The four divergences identify native IncomingMessage.resume(), whose Readable capability inheritance is absent; they are not Socket.resume() failures. The new connection-destroy fixture also exposes missing interpreter HTTP-owned socket close publication. Buffer direct construction and bound-callable null-prototype lookup still fail in the interpreter. The three Luna slices are correcting these concrete causes and Uglify cursor/parser behavior, while the upgrade timeout remains unresolved. Full raw evidence: `target/npm-compat-evidence/cli51-socket-affected.log`. Full 361, matched performance and required gate/stress/contract remain pending.


## Follow-up source and ownership diagnosis

Main advanced through `badd7c89` (verified). Source `c6f1a613` routes direct Buffer construction through its existing public native dispatcher and deletes configurable own bound-function properties through the bound store. The earlier null-prototype lookup repair could not help when deletion silently retained the own constructor. Both remain source-only until the next combined pin.

Root review identified the upgrade liveness hole in `checkpoint_runtime_http`: after UpgradeReady clears runtime_http, that resource is excluded from the known event inventory and queued later UpgradeWriteComplete events are discarded. Those events own the user callbacks and deferred shutdown fence. CLI51's bounded debug probe dispatches only the head/one fences because the one callback never schedules two, consistent with the original three-fence timeout; evidence is `target/npm-compat-evidence/cli51-upgrade-debug.log`. The socket slice is retaining event pollability while outstanding prefix callbacks/fences remain without changing the raw read/close authority.

The new GUI/Taffy main dependency initially failed offline metadata resolution (`post-main-metadata.stderr`). A locked target-specific dependency fetch downloaded taffy 0.14.0; offline locked metadata now passes. No extra release build was spent diagnosing that cache miss. The IncomingMessage repair is being refined to use the shared Readable state for actual HTTP body/EOF delivery, rather than only providing methods over an independent empty compatibility state.


Uglify's CLI51 failure is now traced to Dynamic Function string-literal decoding, not computed-property insertion. Instrumented table output contains ASCII escape digits/letters and marks `a` as forbidden; the lexer at dynamic.c's string branch only unescapes n/r/t. Unicode whitespace source escapes such as \u00a0 therefore become literal u00a0, so `var` tokenizes as v,r. Diagnostic source/binary and retained trace hashes are recorded in `results/uglify-unicode-escape-diagnosis.json`; instrumentation is explicitly not a production package verdict. The execution slice owns a proper lexer repair. IncomingMessage delivery review also requires delaying Agent release until actual shared Readable end emission when body data remains paused/buffered; that ownership refinement is still in progress.


## Integrated Readable/decoder source and CLI52 preflight

Main advanced through `de765af6` (verified). Source `f4f53553`, `4c8f92bd`, `5588f223` routes native HTTP body and EOF through the existing shared TS Readable implementation/state, preserves native EventEmitter listener storage, defers Agent release until actual end listeners complete, and keeps outstanding upgrade fence events pollable after raw ownership transfer. A private ownership completion capability is method 571, not a public C built-in. The new port-zero IncomingMessage pause/read/end fixture passes under Node.

Source `5c3db414` decodes Dynamic Function literals into UTF-16 units, validates malformed escapes at Function construction, and preserves quoted keys/method names. Review corrected buffer sizing, long leading-zero codepoint escapes, raw/escaped Unicode separators and strict context. `fd99e467` also propagates strict mode into the newly introduced lexical block evaluator. Node escape fixtures and diff checks pass. The raw lone-surrogate source-string boundary still needs a scoped source conversion repair rather than lossy UTF-8 conversion.

CLI52 release built in 139 seconds at source `fd99e4670bb4a107460b006f7713ad65c10af18f`, binary SHA-256 `256a7f37b086afed5fc8d729ca17d41f87cb2e7e1dab8f0f6cf7d3ed6cb56450`. Both native preflights were invalid before execution: the escaped non-ASCII path retained a call to a removed local decoder. `de765af6` routes it through the shared UTF-8 decoder. The canonical common/full provider caches finished warming; no package/native pass is inferred from this failed preflight. Evidence: `target/npm-compat-evidence/cli52-build.log`, `cli52-native-preflight.log`, `cli52-node-preflight.log`. The 34-probe combined inventory and the three-package slice await the corrected combined CLI pin.


## CLI53 current targeted verdict

Main publication is verified at `62a5d3d1c0bc853f67ff1e37d12f424bfbda63c3`. The private Dynamic Function source boundary now round-trips raw UTF-16 through scoped WTF-8 in both entry and captured nested source; ordinary UTF-8 runtime conversion remains unchanged. ASCII source uses existing direct byte/string paths without intermediate UTF-16 buffers. These source changes are not a measured speed claim.

CLI53 release built successfully in 20.42 seconds at that exact source, binary SHA-256 `2b6c68cda3dbe5248b326dcda0c638b843d6c5f29a23f0145760074363a68f02`. Both plain callable and Node HTTP native preflights matched one each. Full raw logs and immutable binary are under `target/npm-compat-evidence/cli53-*`.

The affected native selection completed 33 probes: 24 matched, 0 refused, 6 diverged, 3 invalid interpreter references and no deadlines. Upgrade fence, connection-destroy close and Buffer repairs now match. The six divergences are HTTP client preconnect, net half-open, response listener order, response end reentry, IncomingMessage pause/read/end and native bound callable null-prototype constructor lookup. Three interpreter references fail switch function initialization, sloppy Function with-statement parsing and raw unpaired UTF-16 source. Full named outputs: `target/npm-compat-evidence/cli53-native-affected.log`.

The UTC npm slice completed selected 3 / deferred 358 / discovered 361: ws and Arktype pass; Uglify exits successfully but prints undefined code instead of Node's output. Exact phase/source/hash evidence is committed in `results/targeted-fifty-third-three.json`; raw stdout/stderr and binaries remain under `conformance/npm-compat/results/raw/cli53-targeted-three/`. Phase timings include startup and are not warmed throughput. The remaining concrete defects are assigned across the Luna slices and root; no full inventory, final gate, stress/realm or matched HTTP performance verdict is inferred.


## Source follow-ups after CLI53

Normal main pushes are verified through `503f4b9b1e8679dbbedee607a9bb47e91aab0bae`, preserving the concurrent native fetch POST repair `97eab403` in merge `bfed8d3f`. These source follow-ups have not received a new Kiln verdict.

Shared bytecode compilation now initializes every CaseBlock function before case selection, while copying eligible sloppy aliases only when execution reaches each declaration (`7a1e4914`). Ordinary block lists use the same split (`32034f41`). Eligibility is now keyed by each declaration's source span rather than by name (`503f4b9b`); an eligible sibling must not authorize a blocked, async or generator declaration. Added Node-valid fixture assertions cover early block exit, lexical value availability and mixed sibling eligibility. The native bound callable deletion repair and separate sloppy CommonJS direct-eval probe are integrated as `3450126e`. The original `.js` with assertion ran as strict ESM and was invalid against Node; no strictness rule was weakened.

Immutable CLI53 HTTP traces show client ResponseHead/Data/End delivery completes even for the empty-output references. Their missing final output is now localized to server-close completion/liveness, rather than to HTTP body delivery. Uglify's one diagnostic entry exposes caught `ReferenceError: Variable is not defined` results behind the undefined code. That diagnostic is not a production parity pass. Owning Luna slices are addressing HTTP close/reentry, lossless source transport and the Uglify lookup failure before one combined rebuild.


## Main follow-ups and source fidelity

Direct publication to main is verified through `a408bc5cf1c98c0d7da23706f9d333975c2083db`. HTTP close requests remain live until their terminal event is drained; native HTTP-owned client sockets close through runtime HTTP authority, including end-listener reentry. Retirement and checkpoint cleanup also remove completed close markers. These remain source-only until the next combined pin.

The retained Uglify diagnostic identifies `do` as the unresolved identifier at a valid do-while loop. The dynamic executor now implements do-while, including ordinary and labeled continue/break behavior. Node fixtures cover stacked labels and cross-loop continue ownership. The escaped Euro decoder rewind and raw regexp source materialization preserve original units.

Interpreter dynamic entrypoints now retain exact UTF-16 source through parser transport. Shared HIR/IR source-origin metadata restores nested function reflection and includes the origin in native program identity. Ordinary scalar source retains its direct path. Native dynamic Function construction validates regexp literals through the existing shared provider, including unreachable literals; original-source regexp validation and origin invariants are under final review. No new Kiln parity or performance result is inferred from these publications. CLI53 remains the authoritative measured pin until the combined refresh.


## CLI54 build integration and pending validation

Main is verified through `b57efebbfb9bfd8fc53e0835d02888fb15e75af6`. Follow-ups preserve complete native nested-function source instead of only its executable body, enforce attached-source-map invariants, keep scalar diagnostic display direct, and copy source ranges without scanning their prefixes. Frontend original-pattern Unicode regexp early errors now run before DCE and final lowering. Five current dynamic fixtures pass under Node in one batch (`target/npm-compat-evidence/cli54-node-oracles.log`); this is not Kiln parity evidence.

CLI54's first build stopped on a missing inline-eval Lowerer initializer; the next stopped on a missing StringUnits import in the agent entrypoint. Both integration errors are fixed and published. Complete failure logs are retained as `cli54-build.log` and `cli54-build-retry.log`; no CLI54 binary or package verdict exists yet. Native review additionally found incomplete regex lexical-goal classification after control heads and an unchecked regex token allocation; these are assigned before the next combined build retry. Final corpus, gate, stress/realms, contract and matched performance remain pending.


## CLI54 measured verdict and subsequent source

Release build succeeded in 2m18s at `fe0b2ecc535a58048dccb96679c9b341af97c019`, SHA-256 `526a08046dcacc901cd6e397fe00a2f30f3ba6dc5ed944ed040c966724f74b84`. Complete successful log is `target/npm-compat-evidence/cli54-build-complete.log`; the preceding `cli54-build-final.log` retains the C-string-length contract failure. The validator now uses its recorded token flags length, avoiding an unnecessary C-string scan. Obsolete scalar-only dynamic-source rejection helpers are removed.

Plain native preflight was invalid: the common provider build compiled without HTTP and found three unguarded closing_http_resources removals. Node HTTP full-provider preflight matched one probe. Source `c5c79511` adds the three field guards while preserving unconditional transport release. No broader CLI54 native selection ran.

The UTC npm slice completes 2/3: ws and Arktype pass; Uglify still emits undefined code. Selected 3, deferred 358, discovered 361; report `../results/targeted-fifty-fourth-three.json`. Its single diagnostic build identifies `delete is not defined`, unsupported try/finally and a do-while decrement condition failure; this diagnostic is not package parity. The native dynamic execution slice owns these valid-language defects.

The 13-probe Node/interpreter comparison completed 10 matches, including all five HTTP cases. Full report `../results/cli54-interpreter-oracles.json`, raw output under `target/npm-compat-evidence/cli54-interpreter-oracles/`. Switch construction incorrectly rejects a later lexical binding and a nested with getter/delete update writes to the wrong environment; the shared compiler/interpreter slice owns both. A focused no-build discriminator proves direct raw-surrogate RegExp and Function source matches under all flag modes; template interpolation alone replaced its raw source through String conversion. `f9da37e9` keeps language conversion in StringUnits, including generic string receiver coercion. `80308f7b` supplies complete native constructor/arrow/method reflection source. These source follow-ups are published to main and await the next combined pin. Full 361, final gate, stress/realms, HTTP close contract and matched throughput remain pending.


## CLI55 measured verdict and direct main publication

Main is verified through `89f2c64c`; all reviewed source slices are published, including template abstract ToString, exact arrow reflection boundaries, dynamic delete/finally completions, postfix precedence and scoped Annex B diagnostic filtering. The diagnostic iterator integration error is fixed, with the original failed build log retained. The no-error Annex B visitor fast return was published separately while the measurement checkout remained fixed.

CLI55 source `33d4a855c988c3cac79196a8e96b3685e1fe444d`, SHA-256 `a4b626fb490eed457c54ab039759e183d4a403edabfd4e4f68e73e5e754e9bbd`, built in 2m55s. Both provider preflights match; 39 additional native probes yield 34 matches, one refusal, three divergences and one invalid interpreter reference, without deadlines. Counts and named failures are in `../results/cli55-native-affected.json`; the full raw log remains `target/npm-compat-evidence/cli55-native-affected.log`.

The Node/interpreter selection completes 15/16 matches (`../results/cli55-interpreter-oracles.json`). The remaining getter-deletion update difference follows ECMA-262's original Object Environment Record; Node v26.8.1 instead re-resolves the outer environment. This corrects the earlier description of that interpreter result as an engine defect. The new specification probe exposes the actual remaining native defect: it follows Node's re-resolution rather than retaining the binding base.

The UTC npm slice remains 2/3, with ws and Arktype matching and Uglify producing undefined code. Selected 3, deferred 358, discovered 361; exact phase record `../results/targeted-fifty-fifth-three.json`. No full inventory or warmed performance conclusion follows from this slice. Native block-function initialization, native generic/boxed string coercion, the native binding base, direct eval coverage, the update fixture goal and fresh Uglify diagnosis are assigned for follow-up. Final corpus, gate, stress/realms, HTTP contract and matched throughput remain pending.


## Published source after CLI55

Publication is verified through `f99ceff4`. Generic String receiver dispatch now follows abstract ToString, including wrapper overrides; `toString`/`valueOf` retain internal-slot brand checks. The unchanged update-operator fixture now uses `.cjs` for the same sloppy goal as its Node oracle, with a separate strict `.mjs` probe. Direct eval returns supported single-expression results, including immediate function-expression calls and empty source; unsupported statements still refuse explicitly. Native dynamic with updates retain their initial object binding base, and ordinary block function values initialize on scope entry. Blocks without the function spelling skip the additional allocating inventory. These changes are source-only after CLI55, without a claimed current execution or throughput result.

The fresh retained Uglify diagnostic now catches `TypeError: a boolean is not a function` in all four minify cases; parsing and walking the AST succeed. This diagnostic is not a package pass. Full log/binary: `target/npm-compat-evidence/uglify-cli55-diagnostic.log` and `uglify-cli55-diagnostic.bin`. The builtin slice continues localization before the next coalesced build.

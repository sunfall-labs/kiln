# npm compatibility execution and performance

Status: in-progress

Fix every execution and performance defect exposed by the pinned npm corpus, now 361 packages. Node stdout and exit behavior are the correctness authority. The historical 360-package corpus contains 240 passes and 120 non-passes; those are starting evidence, not fresh measurements.

Work on isolated checkouts with Luna slices for module resolution/cycles, native execution, and builtins/host APIs. Defer builds and tests until source fixes can be integrated and validated in batches. Preserve all package smoke tests and exact dependency pins; fixes belong in runtime/compiler semantics.

Acceptance: full npm corpus without package failures or unexplained timeouts; phase evidence separates native build from binary execution. Performance claims need matched correct outputs and measurements of the relevant work. Run the routine repository gate once the changes are finished, plus allocation/rooting stress where affected. Push integrated changes directly to main as they are ready, as clarified by the user; keep incomplete validation explicit and continue until the final checks pass.

## Current verification (2026-10-02)

The CLI79 routine gate ran against integrated main and failed; its full evidence is retained. The validated npm source batch is being published while Luna slices repair the gate findings. Test262 completed its exact routine inventory, with DataView, Function and Uint8Array below committed floors. Native/host parity, thirdparty timing and whole-program facts also need resolution. This does not constitute final acceptance. See `results/cli79-gate-checkpoint.json`.

Remote main is verified through `b2433361`. The complete CLI77 NativeDev inventory finished all 361 original packages: 358 pass and three string-iterator recursion failures (YAML, Express, Winston). CLI78 fixes the shared canonical iterator delegation and passes those three plus QuickLRU, lru-cache, micromark, Turndown and JOSE in the targeted original Release slice (8 selected, 353 deferred). Iterator/collection probes match ordinarily, under GC stress and at 1/2/4 stressed dedicated-thread realms with no skips; ABI/size and the corrected weak-key C fixture pass. Matched hash reuse improves its deep dictionary lookup workload 2.3038x with correct checksum; the actual HTTP pipeline fixture shows no material gain. The required routine gate, source publication and fresh full361 acceptance on CLI78 remain outstanding. Evidence: `results/cli78-validation-checkpoint.json`.

CLI73 fixes JOSE’s original Release smoke and the actual WebCrypto SHA/HMAC hang by preserving typed scalar phi destinations instead of copying only boxed values. The WebCrypto regression matches ordinarily, under GC stress, and at 1/2/4 stressed dedicated-thread realms; emission unit30 and integration37 pass. Three Release package failures remain: micromark, Turndown and QuickLRU. Remote main is published through `02571c92` before this crypto follow-up. Targeted evidence: `results/cli73-validation-checkpoint.json`.

Remote main is now pushed through `893a6ce4`. CLI72 selects 11 original Release smokes: seven pass, including real socket-backed ws; micromark and Turndown have runtime errors, JOSE times out, and QuickLRU differs. The receiver, bracket-backspace, object-shape, and custom-iterator probes all match ordinarily, under GC stress, and at 1/2/4 stressed dedicated-thread realms. Reviewed emission snapshots pass all 37 integration tests; emission unit tests pass 30. Crypto remains a reproduced SHA loop hang with a typed-phi repair under source review. This is targeted coverage; full acceptance and the final gate remain outstanding. Evidence: `results/cli72-validation-checkpoint.json`.

The next source batch includes the TOML RegExp correction: bracket `\b` lowers to U+0008 at either range endpoint, with all six RegExp unit tests passing (including provider parity over every ASCII code). Native TOML/probe verification awaits the next combined CLI build. Corrected explicit Release checks retain QuickLRU’s output divergence and micromark/Turndown’s runtime failures. Minimal Release diagnostics reproduce repeated object-literal factory receiver loss and JOSE’s unresolved sign Promise; these remain under repair. Earlier diagnostic development-mode successes are not Release evidence. Checkpoint: `results/cli71-prebuild-checkpoint.json`.

CLI70 validates the next shared fixes: **7/7 targeted Release packages pass** (Zod, TypeScript, escodegen, Handlebars, Recast, UUID and Diff; 354 deferred), re-export unit8 passes, and the recursive-call probe matches ordinarily and under GC stress with no refusal/skip/divergence. Intermediate named export demand now survives nested star barrels, and captured recursive typed entries receive their upvalue table on guarded generic fallback. This is targeted verification; remaining CLI69 runtime/timeout/output failures, refreshed full acceptance and the final gate are outstanding. Evidence: `results/cli70-validation-checkpoint.json`.

CLI69’s aligned production targeted slice completes **18 pass, 5 compile errors, 8 runtime errors, one runtime timeout, one output difference, and one Node-baseline failure** among the 34 previous native Release non-passes (327 deferred). UUID, Diff, Express, slice-ansi and fourteen others recover. Zod’s intermediate star-export demand and guarded recursive typed-call signatures remain under repair; runtime failures remain explicit. The unchanged `warning` smoke assumes development logging and fails in Node under production, so it has no Release native verdict. Python oracle-environment3, nested-RegExp unit5 and re-export unit8 pass. All three affected Release probes match ordinarily, under GC stress, and at 1/2/4 dedicated-thread realms under stress, with no skips/refusals/divergences. The batch as a whole fails because package defects remain. Validation: `results/cli69-validation-checkpoint.json`. Evidence: `results/cli69-release-targeted-checkpoint.json`. Nothing here is a full refreshed acceptance or a successful push to main.

CLI68 completes the first full native-release inventory: **326 pass, 7 compile errors, 24 runtime errors, 3 output differences, and one ws Node-baseline failure; 361 unique cases selected/discovered/completed, none deferred**. The Node oracle did not align Release’s production environment, explaining the Express/tiny-invariant differences. Shared re-export, typed-call, nested-RegExp and object-literal barrier fixes are prepared for one integrated targeted batch. This is not acceptance; full evidence is `results/cli68-release-inventory-checkpoint.json`.

CLI66 remains the latest complete native-dev package inventory: **360 pass, one Node-baseline failure (`ws`), 361 unique cases selected and discovered, none deferred**. The original `ws` baseline cannot listen because the host denies sockets with `EPERM`; it was not run natively after that failed oracle. There are no native package failures among the 360 tested cases. This is not 361/361 acceptance. Source `7c40f91bd3fb504fe595f9785199fb79d67d89e7`, CLI SHA-256 `b0721b21521aa8672032097b74fd45bfab06ad7119560c7bed8d5a17a79e6273`; exact report: `results/full-sixty-sixth-2026-10-01.json`. Interpreter203 (one ignored), spread parity and six affected GC-stress probes pass. CLI65 namespace reflection/spread and emission37 evidence still applies to unchanged native source.

CLI68 adds a realm-private ordinal hash index for large ordinary dictionaries. Ordered property records remain authoritative; allocation failure retains linear lookup; candidate/in-block and read-only parallel stores retain their existing path. Targeted verification passes 19 distinct native probes, 10 GC-stress probes, 8 realm probes at counts1/2/4 in scheduler mode and dedicated threads under GC stress, the ownership/collection/prepared-clone unit, and all37 emission checks. Rust1.97.1 CLI SHA-256 `b5db3599e0e535470ba752125df87f3ff6660e58a04f0bcfef7206025d0b940b`; exact evidence: `results/cli68-build-checkpoint.json`. These checks do not refresh the CLI66 full package count.

The Uglify dictionary-index comparison completes four timed runs per binary after two alternating warmup pairs. Every run completes 25 calls with Node's checksum 2612876640. The median warmed 24-call batch is 107171.557 ms on the pre-index CLI67 control and 22183.187 ms on CLI68, a 4.8312x speedup for this workload. The matched captured-environment comparison also completes four timed runs per binary: the median warmed 24-call batch is 49170.129 ms on the CLI66 control and 22119.342 ms on CLI68, a 2.2229x speedup. The bcryptjs workload's median call is 5845 ms with the typed-array read disabled and 4501.5 ms on CLI68, a 1.2985x speedup; all four runs per binary produce Node's deterministic hash checksum 163286659, and the workload verifies the fixed-salt hash, accepted/rejected passwords and rounds=8. The numeric bitwise workload matches Node checksum 1756632504, measuring 0.087891 ms per call on CLI68 versus Node's 0.082031 ms; it has no reviewed optimization control, so no numeric optimization gain is claimed. All three results use warmed in-process measurements and source-matched inputs; exact source/build/binary digests and individual runs are in `results/cli68-warmed-throughput-2026-10-01.json`, with full output retained under `target/npm-compat-evidence/`. These are selected workload results, not aggregate package performance. The CLI68 full npm inventory remains in progress; live HTTP, final gate and remote publication remain outstanding, and the push to `main` still fails because github.com cannot resolve.

The checkpoints below are historical and do not supersede the current verification above.

CLI66 full inventory completed all 361 cases on frozen runtime `7c40f91b` (binary `b0721b...`): 360 passes and one `node-baseline-failed` (`ws`: actual Node listen `EPERM`). All six previous native package failures pass; no native compile/runtime/timeout failures occurred among cases whose Node oracle ran. This is not 361/361 acceptance. The first warmed Uglify workload attempt exposed an invalid benchmark option (corrected and Node-verified); the corrected workload exposes a native Release multiplication-parse error, now under active shared-runtime diagnosis. No warmed gain is claimed. Source-matched control GC-stress4 passes after control-only type/status corrections. Evidence: `results/full-sixty-sixth-2026-10-01.json`; complete raw streams remain in managed `conformance/npm-compat/results/raw/cli66-full-361`.

CLI66 (`7c40f91b`, binary SHA-256 `b0721b21521aa8672032097b74fd45bfab06ad7119560c7bed8d5a17a79e6273`) passes interpreter203 (one ignored), corrected spread parity and all six selected native GC-stress probes without refusal or skip. The full 361 inventory refresh is running on this frozen source; no complete refreshed total is claimed yet. The recreated Uglify control (`4f0cb6c4`) has identical tracked sources except reviewed rooted hydration in `dynamic.c`; its CLI build and later stress/performance checks remain pending. See `results/cli66-build-checkpoint.json`.

CLI65 (`cc255315`, SHA-256 `1958eb0143289d73eeca258fb6eee6ea4c6bd4b103e7526174821dd6ffbbd8ce`) passes interpreter203 (one ignored) and emission37. Both namespace reflection and namespace spread probes match; the spread probe is invalid in the interpreter on newly added frozen-array deletion assertions, while its direct native run passes every assertion. Interpreter counterpart repair remains pending. HTTP close authority/scripted/provider/emission seams pass but the live server seam fails before listen; an actual AF_INET socket check records `EPERM`, so live HTTP correctness/performance remains unverified here. No refreshed full count or warmed performance claim is made. See `results/cli65-build-checkpoint.json` and `results/cli64-http-close-checkpoint.json`.

CLI64 (`08c13295`, binary SHA-256 `b1257827cea57da7f74dff17a86ae3e3f345ff23e28f7b2c2bfda971806b24e3`) passes the original CSS-tree smoke test. IR69 and emission37 pass, as do both preflights. The five affected native probes finish with three matches and two divergences: typed-array spread mutation and namespace duplicate keys/interpreter live bindings. These remain active shared-path repairs. No refreshed full total or warmed performance claim is made. Evidence: `results/cli64-build-checkpoint.json` and `results/cli64-css-tree.json`.

CLI63 (`78cdef81`, binary SHA-256 `d66fd7beb5b0c31dd2002b249098729b3b6b4e894dc358dc1e022cd1de1bde6b`) passes the targeted original Zod and dinero.js tests. CSS-tree remains the sole failure in this three-package slice: an internal `CELL` reaches `SpreadObject`. The five-probe native batch completes with two matches and three divergences: TDZ error ordering, builtin spread descriptors and array spread mutation semantics. Focused graph/reparse/interpreter-spread/emission checks pass 35/2/4/37 tests respectively; both native preflights match. Evidence: `results/cli63-build-checkpoint.json` and `results/cli63-targeted-three.json`. No new full-corpus total or warmed performance claim is made.

Latest targeted evidence is CLI62, source `61577503`, binary SHA-256 `3efbedf3957b6cb7da80eb690c745fba525d7b2ebe79517933b09e4c84a27ea2`. Both provider preflights matched. The five-probe native batch has three matches, one divergence (captured immutable logical assignment), and one invalid interpreter probe (throwing object-spread getter). The three-package refresh completed all selected cases with Zod, css-tree and dinero.js still failing; it deferred 358 packages. CLI61 had separately verified cron-parser and EJS. The latest full inventory remains CLI57's 355/361; no newer full total is claimed. See `results/cli62-build-checkpoint.json` and `results/cli62-targeted-three.json`.

Ready commits are collected on the publication branch for direct normal pushes to `main`, as authorized. The last successful remote publication is `f46a32ab`; subsequent publication attempts fail with `Could not resolve host: github.com`. Local source work continues while remote access is unavailable. Getter exception handling and imported-export origin preservation repairs are committed after CLI62 and await the next coalesced validation.

The launcher correction passes seven targeted re-export tests. Percent-encoded relative ESM resolution, canonical package metadata, launcher argv declaration, and createRequire discovery for URL objects, filenames and imported module factories are on main. The saved-output special-path loader fixture now passes both NativeDev and NativeRelease after the shared UTF-16 prefix/suffix repair (`2ceaf061`). The original Release diagnostic proved `filename.startsWith("/")` returned false while its first code unit was 47; no createRequire call-site workaround was introduced. The optimized string probe covers mixed encodings, positive/negative matches, empty/long patterns and partial surrogate pairs. CLI60 is built from `c8e59b99` (SHA-256 `e9bfb9260e55c076382ccf46065c9bf5dc13975c4590fca7443b17a1027cb7b2`, Rust release build 59.72 seconds); both provider preflights pass. The affected native batch is 39 matched, 10 diverged and three invalid out of 52 selected. The nine-package slice completes with Axios, Uglify, ws and Arktype passing; Zod, cron-parser, css-tree, dinero.js and EJS still fail. These are targeted results, not a refreshed full-corpus count. Evidence: `results/cli60-validation-checkpoint.json` and `results/cli60-targeted-nine.json`. Retained fixture log: `target/npm-compat-evidence/cli60-url-prefix-repair-fixture.log` (one test passed, 19.33 seconds).

CLI58 and CLI59 Rust release builds succeeded, but both native provider preflights failed C compilation. No package batch ran for either CLI. CLI58 unity/parser defects were repaired and its focused backend C fixture passed; CLI59 exposed a generated-launcher codec visibility defect, since corrected before the seven targeted re-export tests. Exact source and binary pins, failures and retained log paths are recorded in `results/cli58-cli59-preflight-failures.json`. These invalid preflights do not supersede the last complete package counts below.

CLI57 (`6e621e4f`, SHA-256 `703ddc420faa36f2b14ab75e29dde05180ac68f867245042c80a0335c221491d`) built successfully in 52.14 seconds. Both common and Node provider preflights matched one probe each. The UTC package refresh selected 3 / deferred 358 / discovered 361: Uglify, ws and Arktype all match Node. Exact report: `results/targeted-fifty-seventh-three.json`. Uglify's native phase took 34.686 seconds versus Node's 0.114 seconds; these include startup and are not warmed-throughput measurements. Correct output now permits investigating that execution cost.

The remaining affected native batch completed 41 probes: 40 matched, no refusals or invalid interpreter references, and one divergence, without deadlines. Combined with the two provider preflights, this is 43 affected probes and 42 matches. The String conversion assertions now pass in the interpreter but still fail natively; a narrowed diagnostic is assigned. The new String symbol-protocol fixture matches in both engines. Named evidence: `results/cli57-native-affected.json` and `target/npm-compat-evidence/cli57-native-affected.log`.

The full refresh completed all 361 selected packages with none deferred: 355 pass, six runtime errors, no build failures or timeouts. Failures are Zod, Axios, cron-parser, css-tree, dinero.js and EJS. Exact evidence: `results/full-fifty-seventh-2026-10-01.json`; retained raw streams: `conformance/npm-compat/results/raw/cli57-full-361`. These counts apply to the frozen source/CLI pin above. A bounded GDB sample of the correct Uglify executable confirmed captured-environment hydration in the hot stack (`dyn_scope_set` through property storage), supporting the pending borrowed-environment optimization. That fix requires semantic review and matched measurements before any performance claim. Fresh pending return/throw objects in the finally fixture await collector stress; ordinary parity alone does not prove rooting.

CLI55's 15 of 16 Node/interpreter matches remain historical evidence in `results/cli55-interpreter-oracles.json`. The remaining getter-deletion with-binding corner is governed by ECMA-262's original binding object; Node v26.8.1 re-resolves an outer binding. The specification fixture remains explicit, and original package smokes remain unchanged. No fresh CLI57 Node/interpreter batch is claimed.

Matched bcrypt typed-read and yargs RegExp-cache comparisons are committed under `conformance/npm-compat/results/`. They establish specific correct-output phase-time improvements, not warmed throughput for the whole inventory. The HTTP comparison harness now retains load-client provenance, validates complete pipeline batches and emits per-workload paired summaries; the actual matched pipeline/nonpipeline comparison remains pending. A failure-free full 361-package run on the finished source, the final gate, affected stress/realm checks and HTTP close contract remain outstanding.

## Next integrated batch

After CLI57 measurement, native String concat/padding repairs preserve exact UTF-16 units and checked coercion order, including skipped fillers and oversized target lengths. Native dependent modules now retain distinct import-meta identities for relative `createRequire` bases; the executable entry selects its own resolver table through its relocated path. Dynamic calls now borrow captured cells rather than hydrating a binding table on every invocation. Ordered captured records retain with-object identity and lexical precedence; review corrected private-slot lookup, delete/TDZ behavior and collector fences. Focused regression fixtures pass their Node oracles. Kiln validation and matched performance measurement are deferred to one coalesced build with the pending namespace fixes; these source changes do not alter the CLI57 counts above.

## Inventory

See `issues/01-baseline-inventory.md` for all historical non-passes. Rows remain open until current-tree evidence verifies them.

## Initial targeted evidence (2026-09-30)

A single release build followed by 18 selected packages (342 deferred) produced
2 passes: TypeScript and deterministically seeded simplex-noise. This is not a full
corpus count. All other selected failures retain complete raw streams under
`conformance/npm-compat/results/raw/20260930T105923Z.1635485/`; the machine report
is copied into `results/targeted-initial-2026-09-30.json`. An earlier attempted batch
failed before compilation on long Unix socket paths; the runner now uses a short
durable cache directory and that failed attempt remains separately retained.

Integrated source slices awaiting current CLI package validation:
- Node host bootstrap enabled for every native corpus build; previous targeted results omitted `--node` and remain historical evidence.
- Resolver source formats and importer-relative createRequire, CommonJS cycle exports, live ESM instantiation and namespace bindings.
- Shared Date parsing, named RegExp capture slots, UTF-16 StringDecoder and Array.join, sparse apply property reads, Array subclass method dispatch.
- ICU locale case mapping, DateTimeFormat with per-instant timezone offsets, console/util/fs compatibility, shared Uint8Array encoding helpers.
- Native WeakRef and FinalizationRegistry collector primitives with job lifetime rooting.
- Shared immutable call-planning tables to remove repeated whole-program metadata copies; split-unit dynamic-loader declarations for large bundles.
- Runner inventory checks, bounded jobs, short durable temporary paths, exact output comparison, per-phase timings and explicit running/completed coverage state.

Focused validation has passed the shared-planning publication regression and CommonJS cycle execution. All three focused ESM instantiation and cycle regressions now pass after preserving export cells, reading namespaces through live bindings, and retaining hoisted declarations used by module bodies. No full-corpus pass count or performance improvement is claimed until current-tree package evidence exists. Keep raw logs in `target/npm-compat-evidence/` and per-run corpus streams in `conformance/npm-compat/results/raw/`.

This initial checkpoint preceded publication. The current verification section above records subsequent main pushes and the checks still required before completion.

## Node-mode targeted evidence (2026-09-30)

CLI source commit `5aa7cc42`, run `20260930T125052Z.1746710`: 14/26 pass,
334 deferred. Verified packages: semver, string_decoder, fresh, moment, jsbi,
itty-router, lz-string, camelcase, change-case, commander, luxon, date-fns-tz,
consola, typescript. All outputs match Node. The 12 non-passes are preserved in
`results/targeted-node-mode-2026-09-30.json` with complete phase records.

TypeScript generated-C was a miss: its module emission took 8.608 s; total emission
9.464 s, C compilation 30.974 s, link 0.298 s, process build 43.920 s and native
execution 0.517 s. The earlier no-Node build took 94.95 s (45.37 s emission), but
source/bootstrap, worker count and caches differ, so this comparison is diagnostic
evidence rather than an isolated performance claim. Prettier now compiles (49.937 s),
but cannot resolve `os` from its bundled require helper at runtime.

## Combined targeted evidence (2026-09-30)

CLI source commit `e0f4878d`, run `20260930T134643Z.1785305`: 5/24 pass,
336 deferred. CAC, util, @noble/hashes, cron-parser and Hono now match Node.
The remaining failures expose additional graph loading, lexical initialization,
array numeric property access, dynamic evaluator and exotic-object store defects;
full streams and phase timings are retained. This subset is not a whole-corpus result.

Six new native regression probes passed: callable legacy emitter/stream constructors,
computed spread keys, indexed Object.assign sources, BigInt string forms and whitespace,
and util inspection quotes (`target/npm-compat-evidence/new-native-eighth-adapter.log`).
The first attempted run lacked the Node adapter for the inspection probe; its extension
was corrected to `.mjs` before the successful batch.

## Combined targeted evidence (thirteenth CLI)

The completed 50-package batch at source `0083b496` passed 18 cases and deferred
310; this is targeted coverage, not a full-corpus verdict. It newly verifies
array numeric property access/writes, hash-sum membership, require of ESM
packages, generator async-iterator fallback and filesystem directory APIs
through their real package consumers. The complete report is
`conformance/npm-compat/results/targeted-combined-thirteenth-2026-09-30.json`;
raw streams remain under `results/raw/20260930T143315Z.1822068/`.

Thirty-two selected packages still failed. Further source fixes for module
namespace/captured-cell liveness, loop lexical cell allocation, callable
reflection, Unicode-set RegExp parsing and generator own properties await
batched validation. The native generator probe also exposed a matching
interpreter storage omission, which has been implemented but not yet rerun.

The next targeted batch at CLI source `7e8a6651` selected 12 packages, passed
7 and deferred 348. `jsonc-parser`, `ansi-styles`, `strip-ansi`, `random-seed`,
`common-tags`, `htmlparser2`, and `css-tree` matched Node. QuickLRU progressed
to wrong output; Zod/yargs still hit captured-binding TDZ, Axios still failed
linking, and CSV parse refused during instantiation. Machine evidence is
`results/targeted-combined-fourteenth-2026-09-30.json`; raw streams are retained
at `results/raw/20260930T150355Z.1849117/`.

The corresponding ten-probe native batch reported 5 matched, 3 refused and
2 divergences, recorded in `target/npm-compat-evidence/new-native-fourteenth-retry.log`.
These exposed descriptor dispatch, wrapper instanceof and interpreter UTF-16
loss; source fixes are being integrated before repeating validation.

## Fifteenth targeted batch (2026-09-30)

Source 1d71c025: 14 selected packages, 5 passed, 346 deferred. Zod, csv-parse,
Noble curves, Cuid2 and Nanoid pass against Node. This is partial coverage.
Remaining failures include callable property descriptors, collection size getters,
Array iterator prototype identity, Date subclass adoption, distinct Reflect.construct
newTarget and yargs' undefined command handler. Complete streams are retained in
`conformance/npm-compat/results/raw/20260930T153921Z.1872030/`; the committed report
is `conformance/npm-compat/results/targeted-combined-fifteenth-2026-09-30.json`.

## Sixteenth targeted batch (2026-09-30)

Source d34ffa62: 22 selected, 0 passed, 338 deferred. Every native program
stopped before package execution on the same bootstrap `GetProperty(return)`
refusal. Distinct native iterator prototypes exposed an incomplete absence rule
at Iterator.prototype. These results establish a shared regression, not 22
independent package regressions. Raw evidence: `results/raw/20260930T162040Z.1892828/`.
The interpreter also failed the new descriptor probe because primitive descriptor
API targets were not boxed; source correction 5516f7e9 awaits validation.

## Seventeenth targeted batch (2026-09-30)

Source 73d2032d: 22 selected, 7 passed, 338 deferred. Shared bootstrap regression
resolved. QuickLRU, dotenv, type-detect, markdown-it, object-hash, source-map-js
and d3-format match Node. Remaining diagnostics and complete streams are retained
under `results/raw/20260930T163221Z.1898596/`. The focused native batch reports
4 matches, 2 explicit refusals and 5 divergences; both engines require further
corrections, so these results do not establish finished compatibility.

## Eighteenth targeted batch (2026-09-30)

CLI source a469094d: 16 selected, 6 passed, 344 deferred. Smol-TOML, core-js,
nunjucks, stream-browserify, lodash.clonedeep and Zod match Node. Ten remaining
failures retain complete streams in results/raw/20260930T171703Z.1918853.
A focused three-probe native batch passed RegExp coercion and found callable-name
override and Date deletion divergences. Date graph-root/layout/property-edge
fixture passed after its representation check was corrected. The full corpus,
matched performance evidence, final gate/stress and push remain outstanding.

## Full eighteenth CLI refresh (2026-09-30)

All 360 pinned packages selected, none deferred: 313 pass, 37 runtime errors,
6 compile errors and 4 output divergences. This is current full-corpus evidence
for CLI source a469094d, not a result for later source fixes. CLI SHA256 is
4227b82a12ddc3a336a724af6b8abaa3efa17e5f089aae5f65f1bcd967978c94.
Complete raw evidence is results/raw/20260930T171935Z.1921509; the machine
report is results/full-eighteenth-2026-09-30.json. Current backlog is
issues/02-current-cli18-failures.md. Forty-seven cases still require correction
and verification; matched performance evidence, final gate/stress and push
remain outstanding.

# 01. Measure what a realm costs and what state is process-global

Status: resolved

Two numbers ADR 0070 rests on, measured before anything changes.

1. **What a realm costs.** For a hello-world native binary and the small-service benchmark: resident
   memory broken down into the bootstrap heap (built-ins, intrinsics, module namespaces), the
   Tokio runtime and its buffers, the HTTP state, and everything else; and the time from process
   start to the first line of user code. Use the SIGUSR1 heap report (`runtime/heap.c`) and the
   Linux profiling setup. The bootstrap heap is what the shared immutable region (08) removes from
   every realm after the first.
2. **What state is process-global.** An inventory of every mutable static in
   `crates/native/kiln-backend-c/src/runtime/*.c` (about 45 on 2026-09-26), every kind of static the
   emitter writes into generated code (about 57), `kiln_namespaces`, and every Rust `static` or
   `thread_local!` in `kiln-runtime` and the provider. For each: what it holds and whether it is
   realm state, a cache, or genuinely process-wide.

## Acceptance

- Both results written into this file's comments, with the commit measured.
- ADR 0070's "How cheap a realm must be" section updated with the starting point.

## Comments

### 2026-09-26: the measurement

Measured at `ba4d1890` (`docs: accept ADRs 0069 and 0070`, origin/main), in a clean worktree, with
a release `kiln` (`mbx build --release -p kiln-cli`) and `--profile native-release` binaries.
Machine: `Linux omarchy 7.2.6-1-cachyos #1 SMP PREEMPT_DYNAMIC x86_64`, AMD Ryzen 7 5800H, 16
hardware threads, 14 GiB, glibc malloc, clang 22.1.8. One run per figure except where a median is
given; a shared laptop, so read the numbers to one significant figure of confidence.

Programs: `hello` is `console.log("hi")`; `hold` is the same plus `setTimeout(() => {}, 3000)` so
that it stays alive to be measured; `serve` is `bench/server/serve.mjs` built with `--node`. Load
is `oha` against `127.0.0.1:18088`, keep-alive, concurrency 16 unless stated.

#### What a realm costs

| figure | hello / hold | serve |
| --- | --- | --- |
| binary size (stripped) | 9.3 MB | 14.3 MB |
| RSS at start | 8.4 MB (`hold`) | 12.5 MB |
| of which file-backed (text, rodata) | 7.2 MB | 10.4 MB |
| of which anonymous (private) | 1.2 MB | 2.1 MB |
| RSS after 10k requests (c=16) | -- | 21.3 MB (c=1: 21.9 MB) |
| RSS after 110k requests (c=16, then c=64) | -- | 24.4 MB, still rising ~2.4 MB per 100k |
| collector: live bytes / objects after bootstrap | 164 KB / 1,626 (no collection yet) | 214 KB / 2,394 (after 1 collection) |
| collector: live bytes after 110k requests | -- | 214 KB / 2,409 |
| threads | 1 | 1 |
| whole process, `hyperfine -N` (`/usr/bin/true` is 0.45 ms) | 2.2 ms | -- |
| process spawn to first user line (median of 100) | 1.4 ms (ms clock, floors, so up to 1.9) | -- |
| realm clock start to first user line (`performance.now()`, median of 100) | 1.10 ms | -- |

The realm's wall clock starts inside `kiln_realm_create`, so 1.1 ms of the ~1.5-1.9 ms before the
first user line is realm creation plus bootstrap (Tokio runtime, the built-ins, the compiled
TypeScript prelude's module body); exec, dynamic linking and relocation are the remaining few
hundred microseconds. I did not split creation from bootstrap inside that 1.1 ms (no perf on the
box without a manual install; gdb breakpoints would distort a millisecond).

Resident memory at start, from `/proc/<pid>/smaps`, attributed with an `LD_PRELOAD` malloc shim
that records a backtrace per live allocation (on an unstripped copy of the same build) and with
the collector's SIGUSR1 line:

| part | hold | serve | class |
| --- | --- | --- | --- |
| binary text + rodata (clean, file-backed) | 5.0 MB | 8.4 MB | (iii) shared already |
| libc, libm, libgcc_s, ld.so (clean) | 2.2 MB | 2.3 MB | (iii) shared already |
| binary `.data.rel.ro` after relocation (dirty) + `.data` | 236 KB | 288 KB | (iii) once per process |
| binary `.bss` touched (of 1.15 MB / 3.24 MB mapped) | 188 KB | 368 KB | (i)/(iv): emitted IC site arrays, method and surface indexes |
| collector nursery regions + tenured slabs (malloc'd, mmap'd) | ~640 KB resident of 0.9 MB | ~1.36 MB resident of 2.1 MB | (i) bootstrap: live data is 164 / 214 KB of it |
| shapes, shape transitions, intern table, dictionaries (malloc, outside the collector) | 181 KB | 295 KB | (i) bootstrap metadata |
| Tokio: signal registry `OnceLock` (process-wide), runtime, JoinSet, tables | ~25 KB | ~27 KB | (ii) |
| io_uring ring (`anon_inode:[io_uring]`) + its bookkeeping | 0 (timer only, no ring) | 84 KB + 5 KB | (ii) |
| launcher (`kiln_cli_run_realm`: argv, stdout buffer) | 28 KB | 28 KB | (iv) |
| main-thread stack | 56 KB | 136 KB | (iv) |

The malloc shim reports 1.16 MB in use for `hold` and 2.47 MB for `serve` at start (glibc's
`malloc_stats` agrees: 2.51 MB); nursery regions are 512 KiB allocations that align a 256 KiB
chunk (3 at `serve`'s start), tenured slabs 128 KiB (4). The node:http, provider and kiln-runtime
Rust structures together are under 10 KB.

Under load, live malloc goes from 2.47 MB to 11.1 MB after 10k requests and stays at 11.3 MB after
210k: 18-19 nursery regions (9.4 MB of allocation, about half touched) and 11 slabs. That is the
nursery budget working as designed (`KILN_NURSERY_BUDGET` 4 MiB, spares kept up to
`budget / capacity + 2` chunks); with `KILN_NURSERY_KIB=512` the same 10k requests end at 16.0 MB
instead of 21.3 MB. The collector's live heap stays at 214 KB throughout. The slow RSS rise past
100k requests (24.4 -> 26.9 -> 29.1 MB over successive 100k runs at c=64) is not live malloc,
which is flat; it is glibc retaining freed memory (fragmentation), and `MALLOC_TRIM_THRESHOLD_=0`
does not change it. So under load a realm's resident size is dominated by (iv), its nursery budget
and allocator retention, not by anything the program holds.

Estimate of a realm's share, `serve` at start (private anonymous memory only, ~2.3 MB):

- (i) bootstrap that a shared immutable region could hold: ~0.5 MB of real content (214 KB of live
  collector objects + ~295 KB of shapes and interned keys), which today costs ~1.7 MB resident
  because bootstrap touches whole nursery chunks and slabs before its first collection. The touched
  part of the emitted IC site arrays (368 KB) is realm state or cache, not bootstrap, but today it is
  a process-wide static.
- (ii) Tokio and I/O: ~0.12 MB (the io_uring ring is 84 KB of it; the rest is small).
- (iii) code: 10.4 MB, file-backed and clean, plus ~0.29 MB of relocated data per process. Already
  shared between processes by the page cache and between realms in a process by construction.
- (iv) other: stack, launcher, and under load the nursery budget (4 MiB by default) and glibc
  retention, which is the largest term once the server has served traffic.

#### What state is process-global

Static reading of the tree at `ba4d1890`; nothing was built for this part. Classes: **(a)** realm
state (holds heap values, shapes or per-realm data; must move into the realm or a realm-owned
thread-local), **(b)** a cache that can be validated per realm (the `kiln_c99_gc_heap` pattern in
`runtime/heap.c:706-740`: a `_Thread_local` trusted only while `kiln_realm_generation()` matches),
**(c)** genuinely process-wide (constants built once, env-derived config, signal and tty state,
provider handles that are never freed). The existing ownership manifest
`C99_MUTABLE_STATE_OWNERS` (`crates/native/kiln-backend-c/src/prelude.rs:4628`) already names an owner for
most prelude statics; its owners are mapped here (Realm and Frame -> a, Constant/Process/
TestFixture -> c) except where the reading below disagrees.

**C runtime, `crates/native/kiln-backend-c/src/runtime/*.c, *.h`: 54 mutable statics.**

| file:line | name | holds | class |
| --- | --- | --- | --- |
| builtins.c:344-397 | `kiln_c99_new_target`, `_inheriting_new_target`, `_typed_status`, `_typed_thrown`, `_adopted_this`, `_frame_args` (:380), `_frame_argc`, `_captured_arguments`, `_frame_is_arrow`, `_frame_arguments_object`, `_frame_mapped_arguments`, `_frame_callee` (12) | per-call registers saved and restored around one generated body (manifest: Frame) | a |
| foundation.c:930-932 | `kiln_c99_ffi_functions`, `_ffi_callbacks`, `_ffi_next_root` (3) | node:ffi function and callback metadata, root ids | a |
| foundation.c:933-937 | `kiln_c99_ffi_callback_status`, `_host`, `_thrown`, `_depth`, `_active` (5) | the callback in flight (Frame) | a |
| symbols.c:62-64 | `kiln_c99_symbols`, `_symbol_count`, `_symbol_capacity` (3) | the symbol registry (heap values) | a |
| stores.c:127 | `kiln_c99_array_unscopables` | a heap object | a |
| web.c:104 | `kiln_c99_active_http_response` | the response being written (Frame) | a |
| arrays.c:246 | `kiln_c99_bytes_cache[8]` | recycled >=64 KiB typed-array blocks (manifest: Realm; could equally be per thread) | a |
| heap.c:3315 | `kiln_c99_root_shape` | the root of the shape tree. **The manifest says Constant, but `kiln_c99_shape_transition` (shapes.c:82) and node.c:565 write transitions into it**, keyed by a realm's interned-key pointers and pointing at calloc'd child shapes: every realm's shape tree hangs off one process-global node | a |
| heap.c:719 | `kiln_c99_realm_cached` (`_Thread_local`) | the realm's heap, trusted under the generation | b, **is the pattern** |
| node.c:544 | `kiln_c99_node_http_layouts` (`_Thread_local`) | node:http key and shape layouts; validated by heap pointer, not generation (the address-reuse hazard the generation exists to avoid) | b, nearly the pattern |
| prelude_types.h:785 | `kiln_c99_proto_epoch` | the prototype-mutation epoch every IC checks; plain, non-atomic | b (the validation word itself; per realm or atomic) |
| stores.c:203 | `kiln_c99_builtin_store_epoch` | epoch for built-in method stores | b |
| arrays.c:702, bigint.c:71 | fn-statics `mask`, `one` | BigInt provider handles for 2^64-1 and 1n, never freed | c (unsynchronised lazy init) |
| symbols.c:148 | fn-static `next_serial` | symbol serial counter | c (not atomic) |
| text.c:161 | fn-static `entries[128]` in `kiln_c99_static_text` | string headers over literal bytes | c (unsynchronised fill) |
| node.c:1296, :1314 | `slash`, `made[9]` (`_Thread_local`) | string values over static text, not heap | c |
| node.c:1137-1139 | `kiln_orig_termios`, `kiln_raw_mode_active`, `kiln_atexit_registered` | tty raw mode | c |
| node.c:1191-1193 | `kiln_c99_node_stdin_request`, `_paused`, `_read_active` | stdin reader (manifest: Process) | c |
| surface.c:79-81 | `kiln_c99_builtin_member_index`, `_len`, `_ready` | sorted surface table (Constant) | c |
| prelude_types.h:371, :397 | `kiln_c99_speculate_mode`, fn-static `rng` | `KILN_SPECULATE` mode, its test RNG | c |
| heap.c:347 | `kiln_c99_gc_stress_checks` | `KILN_GC_STRESS` | c |
| heap.c:553-554 (and :694-695) | `kiln_c99_introspect_pending`, `_armed` | SIGUSR1 request flag | c |
| heap.c:333, :1791; stores.c:187 | `kiln_c99_gc_fallback`, `kiln_c99_keys_check_mode`, `kiln_c99_fixture_method_stores` | test fixtures (TestFixture) | c |

Counts: **(a) 27, (b) 4, (c) 23**. The prelude's C text in `prelude.rs` adds 18 more that the
manifest already lists: the 13 `kiln_c99_diagnostic_*` (prelude.rs:2758-2776; a),
`kiln_c99_native_constructors` (a), and `kiln_c99_method_index`, `_len`, `_ready` and the fn-static
`end` (c).

**Emitted code: 44 kinds of mutable static across about 56 emission points** in
`crates/native/kiln-backend-c/src` and `crates/native/kiln-native/src`. No emitted static is covered by the
ownership manifest, which only matches `kiln_c99_` names, and none calls
`kiln_realm_generation()`; the only generation check reachable from emitted caches is the
built-in path inside the prelude's `kiln_c99_site` (`cached_generation`, boxed.c:50, :1046).

| group | kinds (emitting source) | class |
| --- | --- | --- |
| module state | `{sym}_globals[N]` (lib.rs:3696, :4466), `{sym}_template_sites[N]` (lib.rs:2827), `{sym}_key_{i}` and `ku_keys[N]` interned keys (lib.rs:2711, :2720), `{sym}_keys_ready` (lib.rs:2723) | a (5); keys and templates are reset on `KILN_GC_PHASE_RESET` by `{sym}_gc_roots` |
| graph launcher namespaces (kiln-native) | `kiln_exports_{i}`, `kiln_imports_{i}`, `kiln_namespaces[N]`, `kiln_opentui_namespace`, `kiln_node_ffi_namespace`, `kiln_commonjs_{i}` (lib.rs:5662-5705) | a (6); `kiln_graph_gc_roots` returns at once on RESET, and lib.rs:6722 records that `kiln_namespaces` being process-global is why `KILN_WORKERS` forks |
| shared-publication wrapper (kiln-native) | `__thread` `kiln_shared_namespaces`, `kiln_shared_commonjs`, `kiln_shared_opentui_namespace`, `kiln_shared_node_ffi_namespace` (lib.rs:7214-7223) | a (4), already backed by the realm context and cleared per body |
| BigInt literal handles | `{sym}_bigint_{i}` (lib.rs:2497) | c (1) |
| inline caches | per-op `kiln_c99_site` (12 points in `emit_graph/ops/properties.rs`, `call_method.rs`), hoisted `{sym}_fn_{i}_site[N]` (`emit_graph/body.rs:619`), unit `{sym}_sites[N]` / `ku_sites[N]` (lib.rs:3783, :4592), `kiln_dnp_site` (tail.rs:395), private-field sites (tail.rs:117, :166) | b (5); PIC entries check `kiln_c99_proto_epoch` only |
| receiver rows | `_recv_shape`, `_recv_refused`, `_recv_slot`, and the three `_typed_recv_*` (`emit_graph/body.rs:628`, lib.rs:1429) | b (6), never evicted or reset |
| typed-callee cache | `_t_{k}_fn`, `_shape`, `_row`, `_proto`, `_epoch`, `_upvalues` (`emit_graph/body.rs:289-791`, lib.rs:1751) | b (6); upvalues are read directly on a shape hit |
| literal and spread shapes | `lit_shape` (`emit_graph/ops/mod.rs:1164`, :1210), `kiln_sp_shape_{v}`, `kiln_sp_slot_{v}[N]` (mod.rs:1540) | b (3); `lit_shape` is installed on new objects without comparison |
| graph diagnostic scratch | `text[2048]`, `string`, shared `undefined` (kiln-native lib.rs:5487, :5488, :5845) | c (3) |
| speculation and profiling | `kiln_loop_guard_{blk}`, `kiln_guard_entry_{i}` (`emit_graph/body.rs:1339`, :2543), `_param_tags`, `_return_tag`, `_profile_dumped` (lib.rs:1203-1816) | c (5) |

Counts: **(a) 15, (b) 20, (c) 9** (44). About 20 further kinds are immutable tables (string and
BigInt digit bytes, `ku_protos`, source text, startup indices) and are excluded.

**Rust: 21 mutable items** in `kiln-runtime`, `kiln-native-runtime` and
`kiln-native-provider-ffi` (test-only items excluded).

| crate | file:line | name | holds | class |
| --- | --- | --- | --- | --- |
| kiln-runtime | http_engine.rs:36, :60; lib.rs:61 | `FROM_ENV`, `ENABLED` x2 (`OnceLock<bool>`) | HTTP/3 and `KILN_HTTP_TRACE` switches | c |
| kiln-runtime | uring.rs:124, :182, :253 | `KERNEL`, `FROM_ENV`, `WARNED` | kernel version, `KILN_IO`, warn-once latch | c |
| kiln-native-runtime | lib.rs:616 | `RUNTIME_SOURCE` | the generated launcher C, compile-time only | c |
| provider-ffi | lib.rs:1229-1230 | `NEXT_REALM`, `NEXT_SYMBOL` (`AtomicU64`) | process-unique ids | c |
| provider-ffi | lib.rs:1233-1237 | thread-local `REALMS`, `ACTIVE_REALM`, `ACTIVE_REALM_RC`, `ACTIVE_HEAP`, `ACTIVE_RUNTIME` | the per-thread realm registry and the active realm, heap and `RealmRuntime` (Tokio current-thread runtime + LocalSet) | a |
| provider-ffi | lib.rs:1244 | thread-local `ACTIVE_GENERATION` | the realm generation; `kiln_realm_generation_slot()` (:1253) returns its address, bumped on enter, exit and heap attach | a, the mechanism behind every (b) |
| provider-ffi | ffi.rs:209 | thread-local `STATE` | node:ffi libraries, functions, libffi callbacks with raw C context pointers | a, **per thread, not per realm, and never cleared on realm teardown** |
| provider-ffi | lib.rs:9576 | thread-local `COLLATOR` | last ICU collator, keyed by its full spec | b (content-keyed, realm-independent) |
| provider-ffi | lib.rs:7987, :9436; ffi.rs:210 | thread-local `LAST_ERROR` (Temporal), `RECORDS` (Intl segmenter), `LAST_ERROR` (ffi) | per-call out-parameters | c |
| provider-ffi | lib.rs:9875 | `POOL: OnceLock<Option<rayon::ThreadPool>>` | the private Rayon pool | c, shared by design |

Counts: **(a) 7, (b) 1, (c) 13**. There is no Tokio runtime static: `RealmRuntime` builds its own
current-thread runtime (kiln-runtime lib.rs:924), and `WorkerPool` starts one thread per worker,
each with its own. One dependency static showed up in the memory trace instead: Tokio's
`signal::registry::Globals` `OnceLock` (~24 KB, process-wide, initialised from
`kiln_realm_create`). The launcher C that `kiln-native-runtime` emits also has two plain
`static kiln_string header` buffers (lib.rs:737, :1198) and a global `kiln_cli_last_host_error`
(:938) that two realms would share.

**Totals: (a) 49, (b) 25, (c) 45** across the three sources (C runtime 54, emitted 44 kinds, Rust
21), plus 14 (a) and 4 (c) in `prelude.rs`.

Things the reading turned up that the realm work will trip on:

- `kiln_c99_root_shape` is process-global and mutated per realm (above); with it, the whole shape
  tree (calloc'd, outside the collector) is effectively process state keyed by one realm's
  interned-key addresses.
- `kiln_c99_gc_register_site` (heap.c:1415) sets `site->registered` once and never clears it, so a
  static IC site registered with realm A's heap is never registered with realm B's, and B's
  dead-holder sweep would not clear it. Not confirmed by running two realms.
- The emitted IC site arrays are large and zero-filled: `ku_sites` is 1.1 MB of `.bss` in `hello`
  and the node:http prelude module's `{sym}_sites` is 3.1 MB in `serve`. Only 188 KB and 368 KB
  are touched, but a per-realm copy of the array as declared would cost its full size in virtual
  memory and whatever the realm touches in RSS.

#### What this means for ADR 0070

A realm today costs ~1.2 MB (hello) to ~2.3 MB (small HTTP server) of private memory at start on
top of ~7-10 MB of file-backed code that is already shared, and ~1.1 ms of the ~1.5-1.9 ms to the
first user line. The bootstrap's real content is small, ~0.35-0.5 MB (164-214 KB of collector
objects and 181-295 KB of shapes and interned keys), so a shared immutable region would take
~0.5 MB of content and ~1.5 MB of resident nursery and slab pages out of every realm after the first.
What is left is set by allocation granularity rather than content: the first nursery region
(512 KiB allocated, up to 256 KiB touched) and the first tenured slab (128 KiB), about 0.4 MB,
plus whatever per-realm copy of the emitted caches is chosen (up to 3.1 MB virtual for `serve`)
and ~0.1 MB for Tokio and an io_uring ring. The tens-of-kilobytes target therefore needs, besides
the shared region, a small first nursery chunk and slab that grow on demand, emitted caches that
are allocated per realm lazily (or shared and generation-validated) rather than declared at full
size, and a shared or absent io_uring ring per realm. Under load the realm is dominated by its
nursery budget (4 MiB by default), which is a per-realm tuning question rather than a sharing one.

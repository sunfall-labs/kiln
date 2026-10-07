# Native generated-value rooting audit — 2026-10-04

## Scope and source identity

This is a source-only audit of whether a generated boxed value such as `v98`, when live across an allocation-capable call, has a collector-visible root. It does not identify the cause of the parallel `flatMap` corruption.

The audited worktree is at `a92ef16022c7e526c5b4f5b137d0da6ccae37f32`. The compared root checkpoint is `24206d24835be7e3b477faf594cfe81bc1d8275d`. A source diff of `crates/native/kiln-backend-c/src/runtime/` and `crates/native/kiln-backend-c/src/emit_graph/body.rs` between those commits was empty, so the conclusions below apply to the same native runtime/emitter source in both.

## Source evidence

- `crates/native/kiln-backend-c/src/runtime/prelude_types.h:59` defines `kiln_val` as `uint64_t`. Generated graph locals are emitted as boxed `kiln_val vN` locals (or a boxed `kiln_val` array in compact functions) in `crates/native/kiln-backend-c/src/emit_graph/body.rs`.
- Mid-turn collection is requested from the allocation path: `heap.c:4027` (`kiln_c99_gc_alloc_due`) calls the collection path when due; `heap.c:3905` sets `mid_turn` before collecting. `kiln_c99_gc_collect` scans the C stack when `mid_turn` is set (`heap.c:9565`).
- `heap.c:3556` saves and scans a `jmp_buf` with `setjmp`; `heap.c:3547` then calls the `noinline` `kiln_c99_gc_scan_from_here`, which scans stack words up to the thread stack base. The source comment says this covers spilled callee-saved registers and stack words. `heap.c:3529` treats each word both as a raw address and, when its tag matches, as the payload address of a boxed value; interior lookup resolves it to a live heap block.
- The runtime's explicit `kiln_c99_gc_keep_alive` at `heap.c:3327` is documented for a different case: a C temporary whose last ordinary use is before a later callback, finalizer, or allocation. The generated Graph SSA emitter does not emit a keep-alive call for every `vN`.

## Reasoned conclusion (not runtime proof)

For a boxed value that the generated C function will use after an actual C call, the C compiler must preserve the `uint64_t` value across that call. Under the supported C ABI, a caller-clobbered register cannot be its only surviving location: it must be spilled or held in a callee-saved register. The collector's stack scan and `setjmp` register snapshot are intended to see those locations. Because the boxed value is an integer type, it is not expected to live only in an XMM floating-point register. This makes a missing explicit `gc_keep_alive` for a live generated local an unlikely explanation for the observed corruption.

This is ABI/compiler reasoning combined with inspection of the collector implementation, not a proof from a runtime stress experiment, disassembly, or a captured collection. It also does not guarantee survival of a value the C optimizer can prove dead. In particular, the child value may already have become unreachable and been collected before flattening returns, even if its bytes were correct when first written. The remaining investigation should establish when the child becomes dead and whether the flattened output's traced edges and arena ownership preserve it through later collections.

## Parallel flattening evidence considered

`crates/native/kiln-backend-c/src/runtime/spread.c`, in `kiln_c99_par_flatten`, copies child-array references into the flattened output through `kiln_c99_heap_store_val`. The generated call site sets the mapped result array count after the arena join. Parallel worker arenas are initialized with a heap and budget but leave `candidate_id` zero; the inspected collector's actor-candidate handling therefore does not by itself establish a special generational owner for these worker allocations. These observations narrow the next source/runtime question to child reachability and collection timing; they do not prove those edges are correct in the failing execution.

## Runtime evidence boundary

The diagnostic reports available during this audit show the flattened array count at entry and correct child contents before and after a later sequential `flatMap` in a liveness probe. Instrumented logging can change the failing behavior, and no uninstrumented capture established whether a collection reclaimed the child before flattening returned. Therefore this note records a rooting audit, not a fix or a verified explanation of the corruption.

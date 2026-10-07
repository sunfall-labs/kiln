# Sharding timeout profile instruction-offset attribution

This read-only follow-up uses the retained Samply recording and symbolized AOT binary described
in `README.md`. The six original 5-second timeout windows contain 13,856 samples. Counts below use
only each sample's **top/leaf frame** when that frame is the named symbol; counting every stack
frame would incorrectly attribute caller samples to the dispatch routine.

## Dispatch samples are `Map`/`Set` `forEach`

Of 902 leaf samples in `kiln_c99_invoke_method_dispatch` (6.51% of the six-window samples), 484
land at function-relative offset `0x8884`. The next hottest offsets are `0x886e` (56 samples) and
`0x8875` (43). Disassembly at `0x8884` is the conditional branch in this loop:

```asm
movq    0x8(%rbp), %rax
incq    %r14
addq    $0x28, %r13
cmpq    %rax, %r14
jae     ...
movq    (%rbp), %rcx
cmpl    $0x0, (%rcx,%r13)
jne     ...
```

The record layout identifies the operation. `kiln_collection` begins with its `entries` pointer
and `count`; each `kiln_entry` is `{ kiln_value key, kiln_value value, int dead }`, padded to 40
bytes. The loop starts at byte offset 32, advances by 40, compares the `dead` field at entry offset
32, and for a live entry copies its value and key into the callback arguments before calling
`kiln_c99_call`. This is the `KILN_C99_METHOD_COLLECTION_FOR_EACH` arm in
`runtime/dispatch_part_b.c`, whose documented callback order is `(value, key, collection)`; Set
passes the value as both first and second arguments. The operation identity follows from the
collection/entry layout, live-entry test, callback argument copies, and call site, rather than
from a method-code histogram.

The profile therefore points to repeated Map/Set iteration work in the timeout bodies. It does
not show how much of each timeout's total work is spent there, nor prove that iteration alone
causes the deadline misses. Even eliminating every dispatch-symbol leaf sample has a 6.51% ceiling
in this sampled set. The loop itself is semantically required to visit live entries and permit
mutation during iteration; this evidence does not justify changing that behavior.

## Generic call and closure bodies have no single hot offset

Leaf-PC counts inside the other relevant call symbols are small or spread across their bodies:

| Symbol | Leaf samples | Distinct offsets | Most frequent offset |
| --- | ---: | ---: | ---: |
| `kiln_c99_call_inner` | 172 | 76 | 10 samples |
| `kiln_c99_invoke_closure_body` | 180 | broad/dispersed | 25 samples |
| `kiln_c99_call_val` | 31 | -- | no concentration |
| `kiln_c99_create_argument_list` | 8 | -- | no useful signal |

Thus this profile does not isolate argument marshalling or closure-entry setup as a concentrated
instruction-level target. It also does not provide enough evidence for a dispatch-wide rewrite:
the hot loop is one specific collection operation, not the method-code selection sequence.

## Sharding function 77 is the execution body, not a thunk

The 633 leaf samples attributed to Sharding `fn_77` are PCs inside the ELF function symbol
`kiln_module_path_d26041571cf61f0bad70365407312980_fn_77` (symbol size 250,368 bytes). The source
mapping identifies `Sharding.ts` bytes 34,428–38,039 (3,611 source bytes), and the graph reports
916 IR instructions and 231,049 generated-C bytes. The sampled offsets are widely dispersed: the
most frequent individual offsets contribute only five samples. The apparent source/C size gap is
normal expansion of generic boxed C and inline support paths inside the one emitted function; the
PCs resolve to its body symbol, not a wrapper or external thunk. The broad distribution means this
profile does not identify one source expression in `fn_77` to optimize.

## Limits and reproducibility

Sampled PCs identify the `Map`/`Set` iteration loop, but the profile does not retain method-code
values or callback identities at each sample. The 6.51% dispatch leaf share is an aggregate across
all dispatch work in the six windows. The adjacent passing controls are shorter and are not
matched-work comparisons. No production change follows from this diagnostic.

The profile, symbol table, marker-aligned timeout windows, executable digest, and source/compiler
receipt are retained beside this note. The disassembly can be regenerated from the recorded binary
with `llvm-objdump -d --no-show-raw-insn`; the dispatch symbol begins at `0xca09fa0` and the loop
branch is at absolute address `0xca12824` / function-relative offset `0x8884`.

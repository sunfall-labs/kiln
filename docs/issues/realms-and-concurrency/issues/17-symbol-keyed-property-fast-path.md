# 17. Symbol-keyed properties on the fast path

Status: resolved

Found by issue 14: node 26 keeps four own symbol-keyed properties on every `Event` (five on a
`CustomEvent`), holding the event's state, and kiln now does too for parity. Natively, a
symbol-keyed property read or write takes the object model's generic slow path (about 1.9k
instructions each), which costs +24% instructions per `MessagePort` ping-pong and +31% per one-way
message. Symbol keys are ordinary shape keys (`@@`-spelt, `has_symbol_key` on the shape); make
symbol-keyed own-property reads and writes use the same shape/slot inline caches as string keys, in
both emitted sites and the runtime paths, keeping ADR 0071's realm-independence (a symbol created
at run time is a realm-local key; well-known and registered symbols can be shared).

## Acceptance

- Symbol-keyed own-property reads and writes on shaped objects hit the inline cache; the messaging
  ping-pong and one-way rows at or below their instruction counts before issue 14's event slots
  (2.64G and 2.50G for the fixed workloads in issue 14's comments).
- `scripts/native.sh` 0 diverged.

## Comments

### 2026-09-27, integrator

Also in scope, found by issue 18/19's work: natively, a class method with a symbol key is
enumerable, where node and the interpreter make it non-enumerable. The cause is in the shape
definition path in `runtime/definitions.c` (shapes carry attributes since issue 04 step 3b, but the
symbol-keyed definition path does not pass the method attributes). Fix it with the fast path so
symbol-keyed definitions and reads share one shaped route; add a probe.

### 2026-09-27 -- done (branch `work/symbol-fast-path`)

**Keys.** A symbol's property key is its `@@` spelling, as before, but which table it lives in now
follows ADR 0071. Well-known symbols' spellings (`@@iterator`, ...) are registered as shared keys
before the first realm (`kiln_c99_register_builtin_keys`). `Symbol.for(k)` is spelt
`@@symbol-for:` + the registry key (it was the realm's registry *id*, which differs per realm) and
interned into the shared table (`kiln_c99_intern_shared_key`, whatever realms are alive: only
`Symbol.for` makes that spelling). `Symbol()` keys stay realm-local and pinned. A symbol hands the
property paths its canonical key directly (`kiln_c99_symbol_ikey`: a program symbol carries it; a
well-known one's shared pointer is looked up once per process), so the generic computed paths
(`get_computed`, `set_computed_mode`, `define_computed`) no longer hash and probe per access.

**Sites.** Every emitted `GetComputed`/`SetComputed` site gets one extra word,
`static kiln_c99_symbol_cell` -- a pointer to a `kiln_c99_site` allocated (calloc + CAS publish,
never freed) the first time it learns about a symbol. A symbol key branches to
`kiln_c99_val_read_symbol` / `kiln_c99_val_write_symbol`: own slots, one/two-step prototype paths,
absent paths and assignment transitions, the same answers a named site caches. Because the key
varies, every entry is `keyed` (new byte in `kiln_c99_pic_entry`'s padding): a hit compares shape
*and* `entry->key`, and `kiln_c99_site_remember` matches an existing entry by the pair, so one
shape read under several symbols (or under each realm's own `Symbol()`) keeps several entries.
Realm rules: an entry naming a realm-local key already registers the site with its heap and is
retired at teardown (`kiln_c99_pic_entry_is_local` checks `key`). The one relaxation: a keyed entry
whose key is realm-local may cache a transition from a *shared* shape to a realm-local child (step 4
refused those), because no other realm can present that key pointer while the realm lives. That is
what the first `[kType] = ""` field store of every `Event` needed. Path learning accepts reserved
keys only through the symbol sites (`kiln_c99_pic_learn_path_as(.., symbol=1)`); named sites keep
refusing `@@` keys, and runtime slots (`@@private:`, `@@node:`) never reach the symbol sites.

**Private fields on realm-local shapes.** The event slots also made every `Event` shape realm-local,
and private-field sites cached only shared shapes -- so after issue 14 every `#flags` access hashed
its key again (`kiln_c99_shape_slot_named`). A private site now learns a realm-local shape after
registering with the heap (`heap->private_sites`, `kiln_c99_gc_register_private_site`) and is
retired in `kiln_c99_gc_forget_realm_local` before the shapes are freed. This was about half of the
recovered instructions.

**Enumerability (integrator's comment).** `KILN_C99_ATTR_ENUM_FLIP` on a reserved key now means
"hidden symbol" (`symbol_hidden`), stored `enumerable` always 0; `kiln_c99_define_shaped` gives
every method the flip (private methods keep plain READONLY), `define_accessor_shaped` sets it iff
not enumerable, `kiln_c99_ensure_dictionary` materializes it into `symbol_hidden`, and
`kiln_c99_shape_slot_enumerable` answers 0 for every reserved key. The list-mode method/accessor
paths set `symbol_hidden` too. Fixed with it: class `[s]()`, `[Symbol.iterator]()`, `get [s]()`
were enumerable natively; a literal's `get [Symbol.toStringTag]()` was listed by `Object.keys`,
`for-in` and `JSON.stringify` as `@@toStringTag` (the step 3b "pre-existing" note);
`{ ...Class.prototype }` copied symbol methods; `Object.assign` from a list-mode source dropped
symbol keys and copied accessors as `undefined` instead of calling the getter. `has_symbol_key`
is unchanged and still sends spread/clone of symbol-bearing shapes to the per-key loops.

Probe: `conformance/native/symbol-keyed-inline-caches.js` (node, interpreter and native print the
same bytes, also under `KILN_GC_STRESS=1`; origin/main differs on the enumerability lines). C
test: `symbol_keyed_and_private_sites_learn_realm_local_shapes_and_forget_them`.

**Verified.** `mbx test -p kiln-backend-c --offline`: 190 passed, 1 ignored; emitted_c 36 (ABI
surface and one computed-read snapshot re-recorded; prelude band 53,500 -> 54,000 at 53,891).
`scripts/native.sh`: 266 matched, 1 refused (shadow-realm), 0 diverged.
`KILN_REALMS_COUNTS="1 2" scripts/native-realms.sh` over the new probe, `symbols.ts`,
`symbol-own-keys.js`, `message-port-transfer.mjs`, `web-events-streams.mjs`,
`class-accessor-properties.js`, `private-constructor-static-call.js`, `event-inspect-custom.cjs`:
8 matched, 0 diverged.

**Instructions** (`perf stat -e instructions:u`, native-release, issue 14's fixed workload; its
one-way case is 3 x 20,000 = 60,000 messages, so the issue's 2.50G for 50k is 2.99G here, and
0da8a1c4 was rebuilt and measured on the same harness):

| workload | 0da8a1c4 (pre-event-slot) | origin/main ea4f5a00 | this branch | node 26.8.1 |
| --- | --- | --- | --- | --- |
| ping-pong x20k | 2.643G | 3.352G | 2.592G | 1.49G |
| one-way x60k | 2.989G | 4.021G | 2.905G | 1.62G |
| symbol microbench (2M `new` + 6 symbol accesses) | 35.3G | 35.2G | 14.3G | -- |

Per access in a loop (instructions per iteration, loop overhead ~25, a named read's loop 55):
`Symbol()` read 1017 -> 192 (incl. a string compare), `Symbol.for` read 1101 -> 148,
well-known absent read 2041 -> 241, `Symbol()` store 893 -> 149.

**Left / risks.**
- The symbol hit is a call into the prelude (~30-40 instructions with the entry scan), not inlined
  in the emitted site as a named read's first entry is; a well-known symbol's hit also calls out
  for its canonical key.
- `Symbol.for` keys join the process-wide key table and are never freed (the registries they
  mirror live as long as their realms); a program minting unbounded registered symbols grows it.
  The key spelling changed from the registry id to the registry key.
- A computed site that sees many symbols on one shape, or more than 8 (shape, key) pairs, relearns
  the last entry; with several realms running, replaced entries are retired (capacity, not
  correctness).
- `MessagePort` objects are in dictionary mode, so their private fields still take `own_private`
  (key hash) on every access -- the largest remaining messaging cost that is not a clone; it was
  there before issue 14 too.

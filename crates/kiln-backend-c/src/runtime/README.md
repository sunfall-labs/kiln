# The prelude C, split into domain files

`prelude_runtime.c` and `prelude_core.c` used to be two files of 8.2k and 27k lines. They have
been split into the domain files listed below, as a pure move: the concatenation of each group,
in the order listed, was byte-identical to the original single file it replaced. No behaviour
changed.

One file has been added since: `ryu_tables.c`, which is generated and holds only data. A file
added here is a decision about the *shape* of the prelude rather than an ordinary edit -- the
default place for new C is one of the domain files below -- and this one earns its own because
it is 636 lines nobody reads and nobody hand-edits.

**Order is load-bearing.** C needs a declaration before its use, so the parts of each original
file are concatenated in exactly the order below (see `runtime_parts!` and `core_parts!` in
`../prelude.rs`, which is the only place that assembles them). Do not reorder a file relative to
its neighbors without checking that nothing later in the list depends on something defined only
earlier in it -- that dependency is the reason the cut fell where it did.

The freeze on new built-in implementations (`AGENTS.md`, "Freeze `prelude_core.c`") applies to
every file below that came from `prelude_core.c`, exactly as it did to the one file before the
split.

## `prelude_runtime.c` -> `runtime_parts!()`

| file | contents |
| --- | --- |
| `foundation.c` | provider bridges, forward prototypes, node path and stream answers, the value accessors |
| `heap.c` | the collector: blocks, slabs, the nursery, interned keys, marking, sweeping, mid-turn collection, introspection |
| `shapes.c` | shapes, transitions, dictionary mode, and object creation |
| `platform.c` | AbortSignal, AbortController, TextEncoder, TextDecoder, AsyncLocalStorage |
| `node_support.c` | Buffer, dns, diagnostics_channel, util |
| `arrays.c` | arrays, typed arrays, data views |
| `properties.c` | property storage: slot and list stores, puts, hot keys |
| `generators.c` | generator objects |
| `methods.c` | method tables and names, array prototype methods, CommonJS require |
| `web.c` | entropy, hex, UTF-8 validation, URL, URLSearchParams, querystring |
| `ryu_tables.c` | GENERATED (`scripts/gen-ryu-tables.py`): Ryu's two 128-bit multiplier tables, read only by `numbers.c` |
| `numbers.c` | number formatting: the shortest-round-trip `Number::toString`, fixed, precision, exponential, radix |
| `builtins.c` | built-in names, regexp replacement expansion, arguments objects |
| `globals.c` | the global object, console, process, Atomics, global bindings |
| `bigint.c` | BigInt values |
| `temporal_values.c` | Temporal values, tags and built-in kinds |
| `regexp.c` | RegExp objects and matchers |
| `symbols.c` | symbols, property keys, callable methods, namespaces |

## `prelude_core.c` -> `core_parts!()`

| file | contents |
| --- | --- |
| `surface.c` | the built-in surface table and its index, namespace methods, the backlog |
| `values.c` | value predicates, cells, closures, closure invocation, bind, call |
| `text.c` | numbers, the wall clock, truthiness, text conversion, concatenation, equality, UTF-16 iteration |
| `stores.c` | array indices, prototype objects, callable property stores |
| `proxies.c` | Proxy traps |
| `keys.c` | computed has and delete, own keys, exotic stores |
| `read.c` | property reads: own kinds, get, inline-cache learning |
| `write.c` | property writes: put sites, computed get and set, defineProperty, push |
| `inspect.c` | quoting and console inspection |
| `strings.c` | UTF-16 units, String methods, match results, patterns |
| `node.c` | Node: event emitters, streams, http, fs, net |
| `temporal.c` | Temporal operations |
| `dispatch.c` | argument conversion, DataView methods, sort, invoke_method and the method dispatch |
| `enumeration.c` | iteration, has, delete, own keys, enumeration, proxy spread |
| `spread.c` | object spread |
| `definitions.c` | accessors, descriptors, defineProperty and friends, private names, methods |
| `json.c` | JSON.stringify, and the walk that builds JSON.parse's answer from the tape the provider's reader (`crates/kiln-json`) hands back |
| `dates.c` | Date |
| `collections.c` | Map, Set, WeakMap, WeakSet, Proxy objects |
| `iterators.c` | iterators and iterator helpers |
| `errors.c` | errors, private-name errors, every throw helper |
| `promises.c` | the microtask queue, promises, async functions and generators |
| `constructors.c` | Date parsing, Array and Object constructors |
| `dynamic.c` | the dynamic function evaluator (`new Function`) |
| `calls.c` | construct, super, call, set_prototype, instanceof, typeof |
| `operators.c` | comparison, unary and binary operators, ToPrimitive, checked conversions |
| `boxed.c` | the NaN-boxed fast paths the emitted C calls: reads, writes, elements, calls, JSON, sort |

## The two headers, unchanged

`prelude_types.h` and `prelude_declarations.h` were not split; they were not the files this
change was about, and are still included by name in `prelude.rs`.

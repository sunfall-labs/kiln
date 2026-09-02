# 04 — Per-site slot caches for named access

Status: ready-for-agent
Depends on: 03

## What

Prelude:

- `typedef struct kiln_c99_site { uint32_t own; uint32_t proto; } kiln_c99_site;` with
  `KILN_C99_SITE_NONE`.
- `kiln_c99_get_site(host, target, key, key_len, out, depth, thrown, site)`: on an object
  receiver, hit the own slot (`own < count`, same canonical key, data property) or, when the own
  pointer scan misses, the prototype slot; otherwise `kiln_c99_get` and learn.
- `kiln_c99_put_site(host, target, key, key_len, value, strict, depth, thrown, site)`: hit when
  the own slot matches a writable data property on an object that is neither frozen nor a module
  namespace; otherwise `kiln_c99_put_mode` and learn the own slot.
- Learning skips keys starting with `@@` and the nine bytes `__proto__`.

Emitter:

- One `static kiln_c99_site {symbol}_site_{k}` per `GetProperty`, `SetProperty`,
  `SetPropertyStrict` and `CallMethod` instruction, numbered across the unit in emission order.
- Those four forms call the `_site` entry points with `&{symbol}_site_{k}`.

## Semantics argument (must hold, and is written into the ADR)

A hit returns exactly what the slow path would: keys are unique within an object, so the slot
holding this canonical key is the own property the slow path's first scan would find; the
prototype hit is taken only after proving the own miss and only when the immediate prototype is
an ordinary object, which is the slow path's second step. `@@` and `__proto__` are excluded so the
hidden-slot and relinking rules never see a cached answer.

## Done when

- Snapshots regenerated and reviewed (site declarations plus the four instruction forms).
- `.scratch/perf/probe.mjs` prints the same bytes interpreted and native.

## Comments

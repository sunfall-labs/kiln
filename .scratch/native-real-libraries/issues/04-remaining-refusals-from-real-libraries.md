# The rest of the native refusals, by library

Status: ready

Recorded verbatim from the first native run of the corpus so the one-table work can be checked
against real demand. Several will fall out of it; the ones that will not are noted.

| Library | Refusal |
| --- | --- |
| `ajv` | `New` -- target unsupported by the native constructor model |
| `zod` | `New` -- target unsupported by the native constructor model |
| `dayjs` | `GetComputed` |
| `immer` | `GetProperty(from)` on a builtin |
| `lodash` | `GetProperty(getOwnPropertySymbols)` -- see issue 02 |
| `marked` | `CallMethod(iterator)` |
| `mobx` | `Call` -- callee unsupported by the native callable model |
| `rxjs` | `GetProperty(observable)` |
| `vue-reactivity` | `CallMethod(getOwnPropertyNames)` -- see issue 02 |
| `async-concurrency` | unsupported native emission: the `Performance` builtin |

`async-concurrency` also diverges *interpreted*, on the `[Function: returnSignal]` sentinel; that
is a separate, older problem and not native's.

`Performance` is a decision rather than a gap: decide whether the native backend gets a clock
surface at all before implementing one, since ADR 0015 makes the realm's clock virtual.

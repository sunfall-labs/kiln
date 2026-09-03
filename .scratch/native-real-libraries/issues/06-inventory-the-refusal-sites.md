# Inventory the refusal sites, so a gap is a list entry rather than a surprise

Status: measured; enforcement not yet built

Native gaps have been found one bundle at a time all day: a bundle refuses, someone bisects, one
member lands, the next bundle finds the next gap. ADR 0043 ended that for the *named* surface of
the table-driven built-ins -- 48 backlog rows, each with a reason, checked by a `const` assertion.
This is the other half.

## The measurement

    refusal sites in the C prelude          187
    naming themselves (set a detail)          9
    anonymous                               178

Concentration of the anonymous ones, by enclosing function:

    39  kiln_c99_invoke_method_dispatch     the method switch
    17  kiln_c99_get_own_kind               member lookup
    16  kiln_c99_json_read                  (see caveat)
     6  kiln_c99_gc_unit_roots
     5  kiln_c99_node_event
     5  kiln_c99_node_http
     5  kiln_c99_json_string
     4  kiln_c99_node_fs_complete
     4  kiln_c99_construct

## The caveat, which matters more than the number

**178 overstates it.** `KILN_C99_CONTROL_ERROR` is also an internal status that callers convert
into something else upstream. The sixteen in `kiln_c99_json_read` are exactly that: measured,
`JSON.parse` on `{bad`, `[1,`, `nope`, `{"a":}` and `""` throws `SyntaxError` in both engines, so
none of those sites is a user-reachable gap. `gc_unit_roots` is likewise not something a program
reaches by name.

So the inventory needs a classification, not just a count: **which of these can a program actually
arrive at**, and of those, which name what they lacked. Anyone quoting 178 as "the size of the
backlog" would be wrong, and the first person to check would find that out and trust the rest of
the inventory less.

## Why the anonymous ones cost real time

A refusal that names nothing sends the reader to the wrong place. Today, four separate
investigations went astray on this, three of them recorded here:

- `rxjs` refused with the bare line `native backend refusal`, no operation and no module. The cause
  was that the turn loop's three work functions returned their status and discarded the value
  carrying the diagnostic.
- `marked` reported `CallMethod(iterator)` for what was really `@@iterator` -- the diagnostic's
  name filter drops characters outside a set that excludes `@` -- with `receiver_tag` naming the
  *resolved method* rather than the receiver. Both fields misled.
- `lodash`, `immer` and `mobx` all reported `CallMethod(call)`, which named the borrow rather than
  the borrowed method, so one string covered three unrelated receivers.
- `Object.getOwnPropertyDescriptor(f, "prototype")` reports `operation=Call detail=Call callee is
  unsupported by the native callable model` -- a false statement about a supported callee, because
  a helper that refuses without a detail lets the *caller's* instruction invent one.

## The fix, in the shape that already worked once

Make a reachable refusal without a detail a **build failure**, the way surface drift is. That is the
move that ended the builtin-surface class of bug: not a test someone remembers to run, but an
assertion that cannot be ignored, with the reason written where the gap is.

Sequenced deliberately after the current native work lands, because it touches on the order of a
hundred sites in `crates/kiln-backend-c/src/lib.rs` and would conflict with anything else editing
that file.

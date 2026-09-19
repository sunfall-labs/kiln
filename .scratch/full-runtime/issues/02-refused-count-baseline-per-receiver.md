# Issue 02: the inventory's refused count is a baseline per receiver

Status: ready
Feature: full-runtime
Governing ADR: ADR 0063

`conformance/test262-baseline.json` holds a pass/fail count per chunk so a change that wins forty
rows in one chunk and loses forty in another cannot read as no change. The native surface needs
the same: a committed per-receiver refused count from `native-surface-inventory.json`, compared
by the gate, a rise failing it and naming the receiver, and a deliberate `--update` regenerating
it so the diff is the record.

Done when the baseline file exists, the gate compares it, and the diff of landing a receiver
(say `Date.prototype` 33 -> 0) is that receiver's row and nothing else.

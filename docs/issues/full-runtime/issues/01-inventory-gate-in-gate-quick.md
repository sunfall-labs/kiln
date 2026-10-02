# Issue 01: `native-surface-inventory.py --gate` runs in `gate-quick`

Status: ready
Feature: full-runtime
Governing ADR: ADR 0063

The class the gate fails on -- a member reading `undefined` natively where the interpreter has a
value -- went from 1 row to 111 between 2026-09-03 and 2026-09-19 because nothing ran it. On a
warm cache the full inventory is ~600 native compiles in two minutes; `--only` slices exist if
that is too much for the quick loop, but the whole thing is what catches a receiver nobody
thought to slice.

Done when `scripts/gate.sh`'s quick set runs the inventory with `--gate`, keeps its log under the
run's evidence directory like the other steps, and a deliberately introduced silent `undefined`
fails it.

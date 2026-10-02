# 01. Native `encoding` passes a quarter of what the interpreter passes

Status: needs-triage
Feature: wpt-parity

The first committed WPT baseline (`conformance/wpt-baseline.json`, 2026-10-02, WPT `986e75d7`)
shows the engines agreeing almost everywhere except one directory:

| directory | interpreter | native |
|---|---:|---:|
| `encoding` | 8873/10695 | 2131/10695 |

Every other directory is within a handful of subtests. Over the whole baseline the interpreter passes
13,694 of 30,717 subtests and native 6,935 of 30,711, so this one directory is nearly the entire
difference. Both engines are meant to expose the same surface (ADR 0063), so the gap is native debt,
not a property of the tests.

Start by running only this directory under both engines (`scripts/wpt.py --only encoding`) and
diffing which files pass in the interpreter and fail natively. The likely shapes are `TextDecoder`
labels or legacy single- and multi-byte encodings that the interpreter supports and the native
backend refuses or answers differently.

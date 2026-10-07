# 01. Native `encoding` passes a quarter of what the interpreter passes

Status: closed (fixed 2026-10-02: native passes 8872/10695, interpreter 8873)
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

## Comments

**2026-10-02.** Cause: the engines had two `TextDecoder` implementations. The interpreter used
`encoding_rs`; native used a C stub in `runtime/platform.c` that accepted only UTF-8 labels (any
other threw "The encoding label provided is invalid", 7,951 subtests) and hard-coded `fatal` and
`ignoreBOM` to false. Fixed per ADR 0060: `crates/semantics/kiln-encoding` owns label resolution
and decoding on `encoding_rs`, the interpreter calls it, and native reaches it through two provider
exports (`kiln_text_decoder_label`, `kiln_text_decode`; provider ABI 72) in the realm group, so
every variant carries them. Cost: hello-world native-release grows 21.09 MB to 21.25 MB. Native
`encoding` went from 2,131 to 8,872 of 10,695; the interpreter is unchanged at 8,873.
`TextDecoderStream`, `XMLHttpRequest`, `encodeInto` on shared/odd buffers and `{stream: true}` are
missing in both engines and are not part of this.

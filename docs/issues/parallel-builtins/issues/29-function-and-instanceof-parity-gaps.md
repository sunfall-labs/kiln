# 29. Function, instanceof and bound-function parity gaps

Status: resolved
Scope: general node parity, found by issue 28; outside the concurrency and parallelism plans.

1. `native-release` names `{ n: (0, function () {}) }`'s function `"n"` (with or without a spread);
   likely in the release graph path.
2. `instanceof` against a primitive `prototype` is a `TypeError` in node; both engines answer `false`.
3. Native `Object.getOwnPropertyNames(fn)` lists only the property store (no `length`, `name`,
   `prototype`).
4. `super()` into a bound constructor fails in both engines.
5. Both engines treat a bound function's own prototype as `Function.prototype` instead of its
   target's.

## Acceptance

- A regress case per item recorded from node; both engines match.

## Comments

2026-09-30, `work/parity-followups`. Fixed all five reported gaps in both engines.
Release compression preserves anonymous sequence results; instanceof throws for primitive
prototypes and calls custom Symbol.hasInstance, including bound-target delegation. Native
reflection includes intrinsic function keys. Bound super construction preserves arguments,
new.target and returned objects. Binding captures the target's actual prototype.

Regress r50–r54 and corresponding native-release probes record Node answers. The default
Symbol.hasInstance is recognized by immutable realm-owned identity so native error branding
continues through the ordinary engine check. Prototype reads use ordinary property access,
including lazy creation of an ordinary function's prototype. Final shipping evidence will be
recorded after the gate and collector stress run.

The first full gate exposed a descriptor dependency: complete function own-key inventories
made proxy spread and esbuild's function export copying ask for prototype descriptors.
FunctionProto now carries class-constructor identity from class lowering into both native
closure construction paths (ordinary and compact/shared). Descriptors use that authority for
writability; bound functions describe their own stores and intrinsic name/length. Class
prototype assignment is rejected consistently. r52 additionally checks ordinary, arrow,
class, generator and bound prototype descriptors. The two affected native probes match
Node without a refusal; the emitter snapshot suite passes 37/37.

**Final verification**, code commit `793253e9`: routine gate passed all 10 steps; workspace
2,323 passed in 93 suites; regress 55/55; native 368 matched, one existing ShadowRealm
refusal, zero diverged. Routine Test262 97,180 passed out of 102,522 selected, no chunk
regressions; 116 extended cases deferred. Thirdparty correctness and unchanged performance
baseline passed. Collector stress: 324 matched, one existing refusal, zero diverged, 44
runner-declared stress exclusions. Eight affected probes passed twice at 1/2/16/64 realms;
four passed twice with GC stress at 1/2/16 realms. Durable evidence:
`../evidence/function-constructor-shipping-2026-09-30.json`.

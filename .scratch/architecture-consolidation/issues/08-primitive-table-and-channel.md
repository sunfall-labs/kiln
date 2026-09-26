# 08. The primitive table and `kiln:primitives`

Status: ready-for-agent
Blocked by: 01 (ADR 0065)

Build the mechanism of ADR 0065 with a first handful of primitives, enough for 09 and 10:

1. The typed primitive table in `kiln-builtin-surface`.
2. Generated outputs, checked in and verified by a test: the interpreter trait, the C prototypes,
   the `.d.ts` for `kiln-builtins`.
3. `kiln:primitives` resolves only for built-in sources; user code that imports it gets a refusal
   naming the reason.
4. `Instruction::Primitive { id, argc }` in `kiln-ir`, dispatched by the interpreter through the
   trait and emitted by the native backend as a direct call.

Start with representation primitives the Node modules need (an internal slot read and write on a
built-in object, a host call per `fs` operation already reached through `__kilnNativeModules`).

## Acceptance

- Removing a primitive's implementation from either engine fails to build, not at run time.
- A user program importing `kiln:primitives` is refused at resolution in both engines.
- `scripts/native.sh` and `kiln test conformance/node --node` unchanged.

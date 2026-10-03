// kiln-builtins: whether this realm installs kiln's own members (ADR 0068).
//
// Shared by both interpreter and native execution, and first in the common bootstrap so every
// later part can read it. The engines own the decision -- `KILN_EXTENSIONS=0` at run time, or a
// binary built with `kiln build --no-kiln-extensions` -- and publish it as `__kilnExtensions`,
// which is captured here and deleted before any program runs. Absent means installed: kiln's own
// members are on by default, and a host that does not say otherwise gets them.
//
// Every member installed under this flag must be declared in
// `kiln_builtin_surface::KILN_EXTENSIONS`, and a test over this crate holds the two together.
const kilnExtensions: boolean = (globalThis as any).__kilnExtensions !== false;
delete (globalThis as any).__kilnExtensions;

// The realm's engine-private object (`__kilnPrivate`): the one channel between the engine, this
// bootstrap and the builtin modules, captured here and removed from the global object before any
// program line runs. The engine's own operations (`share`, `sharedIs`, `hostProcessStart`,
// `hostProcessCancel`) are on it only when extensions are installed, and the bootstrap pieces
// publish what the modules need (`messagePort`, `structuredClone`) by assigning onto it. A builtin
// module reads it from `globalThis.__kilnPrivate` only during its own initialization, in a window
// the engine opens and closes around that code; no program ever sees the name, or a symbol key.
const kilnPrivate: any = (globalThis as any).__kilnPrivate ?? {};

// The public Node-compatible property is policy in TypeScript; the private methods only retain
// and report a realm's requested status. Natural completion reads this after the turn loop drains.
const processObject: any = (globalThis as any).process;
if (processObject !== undefined && typeof kilnPrivate.processExitCodeGet === "function") {
  Object.defineProperty(processObject, "exitCode", {
    enumerable: true,
    configurable: true,
    get() { return kilnPrivate.processExitCodeGet(); },
    set(value: any) {
      if (value === undefined || value === null) {
        kilnPrivate.processExitCodeSet(undefined);
        return;
      }
      if (typeof value !== "number" && typeof value !== "string") {
        throw new TypeError("The \"code\" argument must be of type number or string.");
      }
      if (typeof value === "string" && value.length === 0) {
        throw new TypeError("The \"code\" argument must not be an empty string.");
      }
      const code = typeof value === "string" ? Number(value) : value;
      if (Number.isNaN(code)) {
        throw new TypeError("The \"code\" argument must be a number or a numeric string.");
      }
      if (!Number.isInteger(code)) {
        throw new RangeError("The \"code\" argument must be an integer.");
      }
      kilnPrivate.processExitCodeSet(code | 0);
    },
  });
}
// `KILN_ENGINE_PRIVATE_FOR_TESTS=1` (set by the conformance probes that drive the actor protocol
// directly, never by a program) leaves the name readable. Off by default in both engines.
if (kilnPrivate.exposed !== true) {
  delete (globalThis as any).__kilnPrivate;
}

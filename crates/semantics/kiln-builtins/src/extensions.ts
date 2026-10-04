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
// program line runs. Host process hooks are engine operations, available independently of public
// Kiln extensions; this bootstrap owns their shared request framing for both process facades.
// Other bootstrap pieces publish what modules need (`messagePort`, `structuredClone`) on the same
// object. A builtin reads it only during initialization, in a window the engine opens and closes.
const kilnPrivate: any = (globalThis as any).__kilnPrivate ?? {};

const hostProcessStart: (frame: Uint8Array) => any = kilnPrivate.hostProcessStart;
const hostProcessCancel: (id: any) => void = kilnPrivate.hostProcessCancel;

function processBytes(value: any, field: string): Uint8Array {
  if (typeof value === "string") return new TextEncoder().encode(value);
  if (value instanceof Uint8Array) return value;
  throw new TypeError("process.run " + field + " must be a string or Uint8Array");
}

function processFrame(command: string, args: string[], input?: any): Uint8Array {
  const commandBytes = processBytes(command, "command");
  const argBytes = args.map((arg) => processBytes(arg, "argument"));
  const inputBytes = input === undefined ? undefined : processBytes(input, "input");
  const rejectNul = (value: Uint8Array, field: string) => {
    for (let index = 0; index < value.length; index++) {
      if (value[index] === 0) throw new TypeError("process.run " + field + " contains NUL");
    }
  };
  rejectNul(commandBytes, "command");
  for (const arg of argBytes) rejectNul(arg, "argument");
  const size = 24 + commandBytes.length + argBytes.reduce((total, arg) => total + 8 + arg.length, 0) + (inputBytes === undefined ? 0 : inputBytes.length);
  const result = new Uint8Array(size);
  const view = new DataView(result.buffer);
  let offset = 0;
  view.setUint32(offset, 1, true); offset += 4;
  view.setUint32(offset, args.length, true); offset += 4;
  view.setUint32(offset, inputBytes === undefined ? 0xffffffff : inputBytes.length, true); offset += 4;
  view.setUint32(offset, inputBytes === undefined ? 0xffffffff : 0, true); offset += 4;
  view.setUint32(offset, commandBytes.length, true); offset += 4;
  view.setUint32(offset, 0, true); offset += 4;
  result.set(commandBytes, offset); offset += commandBytes.length;
  for (const arg of argBytes) {
    view.setUint32(offset, arg.length, true); offset += 4;
    view.setUint32(offset, 0, true); offset += 4;
    result.set(arg, offset); offset += arg.length;
  }
  if (inputBytes !== undefined) result.set(inputBytes, offset);
  return result;
}

kilnPrivate.processRun = (command: string, args: string[], input?: any) =>
  hostProcessStart(processFrame(command, args, input));

// The public Node-compatible property is policy in TypeScript; the private methods only retain
// and report a realm's requested status. Natural completion reads this after the turn loop drains.
const processObject: any = (globalThis as any).process;
const hostDateNow = Date.now;
const now = () => hostDateNow();
Object.defineProperty(Date, "now", {
  value: now,
  writable: true,
  enumerable: false,
  configurable: true,
});
// Node exposes these clock functions as ordinary JavaScript functions. Keep the host clock
// operation private to the wrappers so reflection, property replacement and spies see the same
// callable surface in both engines, rather than a backend method-id representation.
if (processObject !== undefined && typeof processObject.hrtime === "function") {
  const hostHrtime = processObject.hrtime;
  const hostHrtimeBigint = hostHrtime.bigint;
  function hrtime(time?: any): any {
    return arguments.length === 0 ? hostHrtime() : hostHrtime(time);
  }
  if (typeof hostHrtimeBigint === "function") {
    Object.defineProperty(hrtime, "bigint", {
      value: function bigint(): bigint { return hostHrtimeBigint(); },
      writable: true,
      enumerable: true,
      configurable: true,
    });
  }
  Object.defineProperty(processObject, "hrtime", {
    value: hrtime,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}
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

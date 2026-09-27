// kiln-builtins: node's signal events on `process` (lib/internal/process/per_thread.js's
// `startListeningIfSignal`) and `process.kill`, one implementation for both engines (ADR 0060;
// realms issue 23).
//
// Node listens for a signal while `process` has a listener for it: the first `process.on('SIGTERM',
// ...)` starts a `uv_signal_t`, and from then on the signal is an event -- `(name, number)` to
// each listener, emitted from the poll phase of the loop -- instead of what the signal would
// otherwise do. The runtime (`kiln_runtime::signals`) keeps the process-wide half: a realm
// subscribes and gets an endpoint of the cross-realm ports, every arrival of the signal is a
// message queued on it, and the port wait every engine has wakes this realm. The wait is not
// referenced, as node unrefs its signal handle: a listener alone keeps no program alive.
//
// Node stops listening when the last listener is removed. That needs `removeListener` events the
// engines' native emitters do not all send, so here the subscription lasts until a signal
// arrives with nobody listening; the realm then gives it up and the runtime sends the signal to
// the process again, which then does what it does without a listener. Only the timing differs:
// the signal takes effect when this realm next reaches its event loop.
//
// A `node:worker_threads` worker gets none of this, as in node ("signals are not delivered
// through process.on"): only a main realm installs it.

(function installSignalEvents() {
  const G = globalThis as any;
  const workerOp: ((op: number, ...args: any[]) => any) | undefined = G.__kilnWorkerOp;
  const portWait: ((endpoint: number, referenced: boolean) => { requestId: number; promise: Promise<any> }) | undefined =
    G.__kilnPortWait;
  const queueIo: ((fn: (arg: any) => void, arg: any) => void) | undefined = G.__kilnQueueIo;
  const proc = G.process;
  if (typeof workerOp !== "function" || typeof portWait !== "function" || typeof queueIo !== "function") return;
  if (proc === null || typeof proc !== "object" || typeof proc.emit !== "function") return;

  // `__kilnWorkerOp` numbers (`kiln_runtime::workers` and `kiln_runtime::signals`).
  const OP_TAKE = 3;
  const OP_STATE = 5;
  const OP_SELF = 11;
  const OP_CANCEL_WAIT = 12;
  const OP_SIGNAL_NUMBER = 16;
  const OP_SIGNAL_LISTEN = 17;
  const OP_SIGNAL_UNHANDLED = 18;
  const OP_KILL = 19;
  const STATE_QUEUED = 1;

  // `kiln_runtime::signals::NAMES`, in its order: an index crosses to the runtime, which knows
  // this platform's numbers.
  const NAMES = [
    "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGIOT", "SIGBUS", "SIGFPE",
    "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM", "SIGCHLD",
    "SIGSTKFLT", "SIGCONT", "SIGSTOP", "SIGTSTP", "SIGTTIN", "SIGTTOU", "SIGURG", "SIGXCPU",
    "SIGXFSZ", "SIGVTALRM", "SIGPROF", "SIGWINCH", "SIGIO", "SIGPOLL", "SIGPWR", "SIGSYS",
    "SIGINFO", "SIGEMT", "SIGBREAK",
  ];
  const numbers = new Map<string, number>();

  // This platform's number for a signal name, 0 when it has none (node's `signals[name]`).
  function signalNumber(name: any): number {
    if (typeof name !== "string" || name.length < 4 || name.charCodeAt(0) !== 83 /* S */ ||
        name.charCodeAt(1) !== 73 /* I */ || name.charCodeAt(2) !== 71 /* G */) return 0;
    const known = numbers.get(name);
    if (known !== undefined) return known;
    const index = NAMES.indexOf(name);
    const number = index < 0 ? 0 : (workerOp!(OP_SIGNAL_NUMBER, index) as number);
    numbers.set(name, number);
    return number;
  }

  const ERRNO_NAMES: { [errno: number]: string } = { 1: "EPERM", 3: "ESRCH", 22: "EINVAL", 38: "ENOSYS", 78: "ENOSYS" };

  // Node's `ErrnoException(-errno, syscall)`: `kill ESRCH`, with the negative errno libuv uses.
  function errnoException(errno: number, syscall: string): Error {
    const code = ERRNO_NAMES[errno] || "UNKNOWN";
    const error: any = new Error(syscall + " " + code);
    error.errno = -errno;
    error.code = code;
    error.syscall = syscall;
    return error;
  }

  function codedError(Ctor: any, code: string, message: string): Error {
    const error: any = new Ctor(message);
    error.code = code;
    return error;
  }

  function received(value: any): string {
    if (value === null) return "Received null";
    if (value === undefined) return "Received undefined";
    if (typeof value === "function") return "Received function " + value.name;
    if (typeof value === "object") {
      const name = value.constructor && value.constructor.name;
      return name ? "Received an instance of " + name : "Received an instance of Object";
    }
    let shown = typeof value === "string"
      ? "'" + (value.length > 25 ? value.slice(0, 25) + "..." : value) + "'"
      : String(value);
    if (typeof value === "bigint") shown += "n";
    return "Received type " + typeof value + " (" + shown + ")";
  }

  interface Subscription {
    name: string;
    number: number;
    endpoint: number;
    wait: { requestId: number; promise: Promise<any> } | null;
    scheduled: boolean;
  }
  const listening = new Map<string, Subscription>();

  function wait(entry: Subscription): void {
    if (entry.wait !== null || listening.get(entry.name) !== entry) return;
    const pending = portWait!(entry.endpoint, false);
    entry.wait = pending;
    pending.promise.then(() => {
      if (entry.wait !== pending) return;
      entry.wait = null;
      schedule(entry);
    });
  }

  // Node emits a signal from the loop's poll phase, where libuv reads its signal pipe.
  function schedule(entry: Subscription): void {
    if (entry.scheduled) return;
    entry.scheduled = true;
    queueIo!(deliver, entry);
  }

  function deliver(entry: Subscription): void {
    entry.scheduled = false;
    if (listening.get(entry.name) !== entry) return;
    let arrived = 0;
    while (workerOp!(OP_TAKE, entry.endpoint) !== undefined) arrived++;
    wait(entry);
    for (let index = 0; index < arrived; index++) {
      if (proc.listenerCount(entry.name) === 0) {
        // Nobody listens any more: give the signal back to the process (see the top).
        listening.delete(entry.name);
        const pending = entry.wait;
        entry.wait = null;
        if (pending !== null) workerOp!(OP_CANCEL_WAIT, pending.requestId);
        workerOp!(OP_SIGNAL_UNHANDLED, entry.endpoint);
        return;
      }
      proc.emit(entry.name, entry.name, entry.number);
    }
  }

  // `startListeningIfSignal(type)`, which node runs from `newListener` -- before the listener is
  // added, so a signal that cannot be caught throws and adds nothing.
  function startListeningIfSignal(type: any): void {
    if (listening.has(type)) return;
    const number = signalNumber(type);
    if (number === 0) return;
    const endpoint = workerOp!(OP_SIGNAL_LISTEN, number) as number;
    if (endpoint < 0) throw errnoException(-endpoint, "uv_signal_start");
    const entry: Subscription = { name: type, number, endpoint, wait: null, scheduled: false };
    listening.set(type, entry);
    wait(entry);
  }

  // A signal this process sent itself has already been queued when the runtime's `kill` returns;
  // taking it into this loop iteration's poll phase is node's order, where the signal pipe is
  // readable the next time the loop polls.
  function deliverSentToSelf(number: number): void {
    listening.forEach((entry) => {
      if (entry.number === number && (workerOp!(OP_STATE, entry.endpoint) & STATE_QUEUED) !== 0) schedule(entry);
    });
  }

  // node's `process.kill` (lib/internal/process/per_thread.js).
  function kill(pid: any, sig?: any): boolean {
    // eslint-disable-next-line eqeqeq
    if (pid != (pid | 0)) throw codedError(TypeError, "ERR_INVALID_ARG_TYPE",
      'The "pid" argument must be of type number. ' + received(pid));
    let number: number;
    if (sig === (sig | 0)) {
      number = sig;
    } else {
      sig = sig || "SIGTERM";
      number = signalNumber(sig);
      if (number === 0) throw codedError(TypeError, "ERR_UNKNOWN_SIGNAL", "Unknown signal: " + sig);
    }
    const errno = workerOp!(OP_KILL, pid | 0, number) as number;
    if (errno !== 0) throw errnoException(errno, "kill");
    if (number !== 0) deliverSentToSelf(number);
    return true;
  }
  Object.defineProperty(proc, "kill", { value: kill, writable: true, enumerable: false, configurable: true });

  // Signals are the main realm's: node does not deliver them to a worker's `process`.
  if (workerOp(OP_SELF) !== undefined) return;

  // Every way a listener is added runs node's `newListener` hook first. The engines' `process`
  // emitters do not all emit `newListener`, so the methods that add one run it themselves.
  for (const method of ["on", "addListener", "once", "prependListener", "prependOnceListener"]) {
    const original = proc[method];
    if (typeof original !== "function") continue;
    const wrapped = function (this: any, type: any, listener: any) {
      if (this === proc && typeof type === "string" && typeof listener === "function") startListeningIfSignal(type);
      return original.apply(this, arguments as any);
    };
    Object.defineProperty(wrapped, "name", { value: method, configurable: true });
    Object.defineProperty(proc, method, { value: wrapped, writable: true, enumerable: false, configurable: true });
  }
})();

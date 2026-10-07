// `kiln:process`: kiln's own process API (ADR 0068), separate from Node's `node:process`, which is
// the global `process` object and nothing more. The private host hooks are the only
// engine-specific part; validation, framing, cancellation and public result shape stay here.

// The host's process hooks are on the realm's engine-private object, which is readable only while
// this module initializes and exists with kiln's extensions only: without them this module
// refuses (a TypeError naming it, like every kiln: module in both engines).
const enginePrivate: any = ((): any => { const G = globalThis as any; return G.__kilnPrivate; })();
if (enginePrivate?.extensions !== true) {
  throw new TypeError("kiln:process requires enabled Kiln extensions");
}
const hostProcessRun: (command: string, args: string[], input?: any) => any = enginePrivate.processRun;
const hostProcessCancel: (id: any) => void = enginePrivate.hostProcessCancel;

export interface RunOptions {
  /** Bytes (or UTF-8 text) written to the child's standard input, which is then closed. */
  input?: string | Uint8Array;
  /** Aborting kills the child and rejects with the signal's reason. */
  signal?: AbortSignal | null;
}

export interface RunResult {
  pid: number | undefined;
  stdout: Uint8Array;
  stderr: Uint8Array;
  /** The exit status, or `null` when a signal ended the child. */
  exitCode: number | null;
  /** The signal number that ended the child, or `null` when it exited. */
  signal: number | null;
}

/** Run `command` with `args` (no shell), collecting its output. */
export async function run(command: string, args: string[] = [], options: RunOptions = {}): Promise<RunResult> {
  if (typeof command !== "string") throw new TypeError("process.run command must be a string");
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) throw new TypeError("process.run args must be an array of strings");
  if (options === null || typeof options !== "object") throw new TypeError("process.run options must be an object");
  const signalOption = options.signal;
  const signal = signalOption === null ? undefined : signalOption;
  if (signal !== undefined && (typeof signal !== "object" || typeof signal.addEventListener !== "function" || typeof signal.removeEventListener !== "function")) throw new TypeError("process.run signal must be an AbortSignal");
  if (signal && signal.aborted) throw signal.reason;
  const pending = hostProcessRun(command, args, options.input);
  let onAbort: any;
  let abortPromise: Promise<any> | undefined;
  if (signal) {
    abortPromise = new Promise((_resolve, reject) => {
      onAbort = () => { hostProcessCancel(pending.requestId); reject(signal.reason); };
    });
    signal.addEventListener("abort", onAbort, { once: true });
    if (signal.aborted) onAbort();
  }
  try {
    return await (abortPromise === undefined ? pending.promise : Promise.race([pending.promise, abortPromise]));
  } finally {
    if (signal && onAbort) signal.removeEventListener("abort", onAbort);
  }
}

export default { run };

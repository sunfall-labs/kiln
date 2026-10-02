// `kiln:process`: kiln's own process API (ADR 0068), separate from Node's `node:process`, which is
// the global `process` object and nothing more. The private host hooks are the only
// engine-specific part; validation, framing, cancellation and public result shape stay here.

function bytes(value: any): Uint8Array {
  if (typeof value === "string") return new TextEncoder().encode(value);
  if (value instanceof Uint8Array) return value;
  throw new TypeError("process.run input must be a string or Uint8Array");
}

function rejectNul(value: Uint8Array, field: string): void {
  for (let index = 0; index < value.length; index++) {
    if (value[index] === 0) throw new TypeError("process.run " + field + " contains NUL");
  }
}

function frame(command: string, args: string[], input: any): Uint8Array {
  const commandBytes = bytes(command);
  const argBytes = args.map((arg) => bytes(arg));
  const inputBytes = input === undefined ? undefined : bytes(input);
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
  view.setUint32(offset, commandBytes.length, true); offset += 4; view.setUint32(offset, 0, true); offset += 4;
  result.set(commandBytes, offset); offset += commandBytes.length;
  for (const arg of argBytes) {
    view.setUint32(offset, arg.length, true); offset += 4; view.setUint32(offset, 0, true); offset += 4;
    result.set(arg, offset); offset += arg.length;
  }
  if (inputBytes !== undefined) result.set(inputBytes, offset);
  return result;
}

export interface RunOptions {
  /** Bytes (or UTF-8 text) written to the child's standard input, which is then closed. */
  input?: string | Uint8Array;
  /** Aborting kills the child and rejects with the signal's reason. */
  signal?: AbortSignal | null;
}

export interface RunResult {
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
  const pending = __kilnHostProcessStart(frame(command, args, options.input));
  let onAbort: any;
  let abortPromise: Promise<any> | undefined;
  if (signal) {
    abortPromise = new Promise((_resolve, reject) => {
      onAbort = () => { __kilnHostProcessCancel(pending.requestId); reject(signal.reason); };
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

// State shared by the public actor module and its worker entry within one realm. Keeping it
// here makes their static dependency graph acyclic while ticket identities stay private.
export abstract class Actor {}
// The module graph can evaluate this internal module in more than one dynamic-import plan.
// A global symbol marks the prototype chain across those plans without sharing mutable state.
Object.defineProperty(Actor.prototype, Symbol.for("kiln.actor.base"), { value: true });

export type Ticket = {
  readonly value: any;
  readonly consumed: boolean;
  readonly graph: boolean;
  consume(): void;
  suspend(): any;
  restore(value: any): void;
};
const tickets = new WeakMap<object, Ticket>();
const isolatedMark = Symbol.for("kiln.actor.isolated");
const ticketMark = Symbol.for("kiln.actor.ticket");
const ISOLATE = 26;
const ISOLATED_HAS = 27;
const CANDIDATE_BEGIN = 28;
const CANDIDATE_END = 29;

function isolatedOp(operation: number, ...args: any[]): any {
  const port = (globalThis as any).MessagePort;
  const workerOp = port?.[Symbol.for("kiln.messagePort")]?.workerOp;
  if (typeof workerOp !== "function") throw new Error("kiln:actors requires enabled worker messaging");
  return workerOp(operation, ...args);
}

function isIsolated(value: any): boolean {
  return value !== null && typeof value === "object" && value[isolatedMark] === true
    && isolatedOp(ISOLATED_HAS, value) === true;
}

declare const isolatedBrand: unique symbol;
declare const transferBrand: unique symbol;
export type Isolated<T> = { readonly [isolatedBrand]: T };
export type TransferTicket<T> = { readonly [transferBrand]: T };

function dataCloneError(message: string): Error {
  const error = new Error(message);
  error.name = "DataCloneError";
  return error;
}

export function isolate<T>(factory: () => T): Isolated<T> {
  if (typeof factory !== "function") throw new TypeError("isolate requires a factory");
  const handle: any = { __uncloneable: () => {} };
  // Resolve the native operation before opening the candidate allocation stream. Looking
  // up this symbol, and the rest/spread bridge in `isolatedOp`, allocate unrelated blocks.
  const port = (globalThis as any).MessagePort;
  const candidateWorkerOp = port?.[Symbol.for("kiln.messagePort")]?.workerOp;
  if (typeof candidateWorkerOp !== "function") throw new Error("kiln:actors requires enabled worker messaging");
  // Native realms put this synchronous factory's nursery allocations into fresh, tagged
  // chunks. A nested factory or a realm without a nursery still works, but cannot claim an
  // independently movable graph. The engine seals the chunks even when the factory throws.
  const candidate = candidateWorkerOp(CANDIDATE_BEGIN) === true;
  let value!: T;
  try {
    value = factory();
  } catch (error) {
    if (candidate) {
      // Cleanup must not replace the factory's original exception. The engine may already
      // have invalidated candidate provenance after a collection or failed allocation.
      try { candidateWorkerOp(CANDIDATE_END, false); } catch {}
    }
    throw error;
  }
  if (candidate && candidateWorkerOp(CANDIDATE_END, true) !== true) {
    throw new Error("could not seal isolated graph candidate");
  }
  if (candidateWorkerOp(ISOLATE, handle, value) !== true) throw new Error("could not retain isolated graph");
  // Dynamic import plans may evaluate this module separately in one realm. This marker only
  // identifies a handle for fail-closed graph transfer; it must never expose the graph itself.
  Object.defineProperty(handle, isolatedMark, { value: true });
  return handle;
}

export function transfer<T>(value: Isolated<T> | ArrayBuffer): TransferTicket<T> {
  const markedHandle = value !== null && typeof value === "object"
    && (value as any)[isolatedMark] === true;
  const graph = isIsolated(value) || markedHandle;
  if (!graph && !(value instanceof ArrayBuffer)) {
    throw new TypeError("transfer requires an isolated handle or ArrayBuffer");
  }
  const ticket: any = { __uncloneable: () => {} };
  let used = false;
  let retained: any = value;
  let suspended = false;
  const entry: Ticket = Object.freeze({
    // The ticket marker keeps this entry alive across dynamic-import plans. Its value must
    // therefore be a revocable slot, not a nonconfigurable data property holding the handle.
    get value() { return retained; },
    graph,
    get consumed() { return used; },
    consume() { used = true; retained = undefined; suspended = false; },
    suspend() {
      if (!graph || used || suspended) throw dataCloneError("transfer ticket is unavailable");
      const handle = retained;
      retained = undefined;
      suspended = true;
      return handle;
    },
    restore(handle: any) {
      if (used || !suspended || !isIsolated(handle)) {
        throw dataCloneError("transfer ticket could not be restored");
      }
      retained = handle;
      suspended = false;
    },
  });
  tickets.set(ticket, entry);
  Object.defineProperty(ticket, ticketMark, { value: entry });
  return ticket;
}

export function prepareActorMessage(
  value: any, buffers: ArrayBuffer[], consumed: Ticket[], seen = new Map<any, any>(),
  onGraphTicket?: (handle: any, ticket: Ticket) => any,
): any {
  if (value === null || typeof value !== "object") return value;
  // The symbol bridges separate dynamic-import plans, but user code can forge a symbol
  // property. Treat it only as a hint: object graphs are refused until the runtime proves
  // ownership, and the only accepted payload is an actual transferable ArrayBuffer.
  const localTicket = tickets.get(value);
  const ticket = localTicket || value[ticketMark];
  if (ticket) {
    if (typeof ticket !== "object" || ticket === null) throw dataCloneError("invalid transfer ticket");
    if (ticket.consumed) throw dataCloneError("transfer ticket was already consumed");
    if (ticket.value === undefined) throw dataCloneError("transfer ticket is unavailable");
    if (ticket.graph) {
      // A symbol property is only a cross-plan hint and can be forged. Moving a graph needs
      // the ticket issued by this module's private WeakMap until the engine authenticates
      // cross-plan tickets itself.
      if (onGraphTicket && localTicket === ticket) return onGraphTicket(ticket.value, ticket);
      throw dataCloneError("graph transfer needs ownership proof");
    }
    if (!(ticket.value instanceof ArrayBuffer)) throw dataCloneError("transfer ticket has no ArrayBuffer");
    buffers.push(ticket.value);
    consumed.push(ticket);
    return ticket.value;
  }
  if (isIsolated(value) || value[isolatedMark] !== undefined) {
    throw dataCloneError("isolated handle needs transfer()");
  }
  if (seen.has(value)) return seen.get(value);
  if (Array.isArray(value)) {
    const copy: any[] = [];
    seen.set(value, copy);
    for (let index = 0; index < value.length; index++) if (index in value) copy[index] = prepareActorMessage(value[index], buffers, consumed, seen, onGraphTicket);
    return copy;
  }
  // Preserve special cloneable objects and their own semantics. Tickets nested in those objects
  // cannot be found without invoking user accessors, so the serializer will reject them itself.
  if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return value;
  const copy: any = {};
  seen.set(value, copy);
  for (const key of Object.keys(value)) copy[key] = prepareActorMessage(value[key], buffers, consumed, seen, onGraphTicket);
  return copy;
}

export function consumeActorTickets(consumed: Ticket[]): void {
  for (const ticket of consumed) ticket.consume();
}

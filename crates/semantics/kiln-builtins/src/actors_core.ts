// State shared by the public actor module and its worker entry within one realm. Keeping it
// here makes their static dependency graph acyclic while ticket identities stay private.
// Structured clone owns the shared-value predicate for every message path. Its private bridge
// is installed by the bootstrap before this module loads. Importing kiln:shared here would make
// that extension's refusal run before kiln:actors can give its own when extensions are disabled.
const nativeIsShared: ((value: any) => boolean) | undefined =
  (globalThis as any).structuredClone?.[Symbol.for("kiln.structuredClone")]?.isShared;

/** Base class of every actor: `spawnActor` constructs a subclass in its own realm. */
export abstract class Actor {}

/** @internal */
export type Ticket = {
  readonly value: any;
  readonly consumed: boolean;
  readonly graph: boolean;
  consume(): void;
  suspend(): any;
  restore(value: any): void;
};
const tickets = new WeakMap<object, Ticket>();
// Marks, in a message walk's `seen` map, that the message already carries a graph ticket.
const graphInMessage = {};
// The interpreter snapshots into its own heap. Native prepared handles own an off-heap arena
// instead and never enter this table.
const preparedSnapshots = new WeakMap<object, any>();
const hasOwn = Object.prototype.hasOwnProperty;
const isolatedMark = Symbol.for("kiln.actor.isolated");
const ticketMark = Symbol.for("kiln.actor.ticket");
const ISOLATE = 26;
const ISOLATED_HAS = 27;
const CANDIDATE_BEGIN = 28;
const CANDIDATE_END = 29;
const PREPARE = 37;
const PLAIN_MESSAGE = 38;

function isolatedOp(operation: number, ...args: any[]): any {
  const port = (globalThis as any).MessagePort;
  const workerOp = port?.[Symbol.for("kiln.messagePort")]?.workerOp;
  if (typeof workerOp !== "function") throw new Error("kiln:actors requires enabled worker messaging");
  return workerOp(operation, ...args);
}

function isIsolated(value: any): boolean {
  return value !== null && typeof value === "object" && value[isolatedMark] === true
    && (preparedSnapshots.has(value) || isolatedOp(ISOLATED_HAS, value) === true);
}

declare const isolatedBrand: unique symbol;
declare const transferBrand: unique symbol;
export type Isolated<T> = { readonly [isolatedBrand]: T };
export type TransferTicket<T> = { readonly [transferBrand]: T };

function dataCloneError(message: string): DOMException {
  return new ((globalThis as any).DOMException)(message, "DataCloneError");
}

function checkPreparedData(value: any, seen = new Set<any>(), depth = 0): void {
  if (value === null || value === undefined || typeof value === "number" ||
      typeof value === "string" || typeof value === "boolean") return;
  if (typeof value !== "object" || depth > 512) {
    throw dataCloneError("prepareTransfer accepts only plain data graphs");
  }
  if (seen.has(value)) return;
  seen.add(value);
  const array = Array.isArray(value);
  const prototype = Object.getPrototypeOf(value);
  if (array ? prototype !== Array.prototype
            : prototype !== Object.prototype && prototype !== null) {
    throw dataCloneError("prepareTransfer accepts only ordinary objects and arrays");
  }
  const keys = Reflect.ownKeys(value);
  for (const key of keys) {
    if (array && key === "length") continue;
    if (typeof key !== "string") throw dataCloneError("prepareTransfer cannot copy symbol keys");
    if (array) {
      const index = Number(key);
      if (!Number.isSafeInteger(index) || index < 0 || String(index) !== key ||
          index >= value.length) {
        throw dataCloneError("prepareTransfer cannot copy array properties");
      }
    }
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    if (!descriptor || !descriptor.enumerable || !descriptor.configurable ||
        !descriptor.writable || !("value" in descriptor)) {
      throw dataCloneError("prepareTransfer cannot copy this property descriptor");
    }
    checkPreparedData(descriptor.value, seen, depth + 1);
  }
}

/** Run `factory` and hold its graph so `transfer` can move it to an actor without a copy. */
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
  try {
    if (candidateWorkerOp(ISOLATE, handle, value) !== true) throw new Error("could not retain isolated graph");
  } catch (error) {
    if (candidate) {
      try { candidateWorkerOp(CANDIDATE_END, false); } catch {}
    }
    throw error;
  }
  // Dynamic import plans may evaluate this module separately in one realm. This marker only
  // identifies a handle for fail-closed graph transfer; it must never expose the graph itself.
  Object.defineProperty(handle, isolatedMark, { value: true });
  return handle;
}

/** Snapshot a plain data graph for an actor call; `value` stays usable. */
export function prepareTransfer<T>(value: T): Isolated<T> {
  if (value === null || typeof value !== "object") {
    throw dataCloneError("prepareTransfer requires an ordinary object or array graph");
  }
  const handle: any = { __uncloneable: () => {} };
  const prepared = isolatedOp(PREPARE, handle, value);
  if (prepared === null) {
    // The interpreter has no movable GC arena. Its private snapshot stays behind the handle,
    // then ordinary message cloning establishes the receiver's independent ownership.
    checkPreparedData(value);
    preparedSnapshots.set(handle, structuredClone(value));
  } else if (prepared !== true) {
    throw dataCloneError("prepareTransfer cannot snapshot this graph");
  }
  Object.defineProperty(handle, isolatedMark, { value: true });
  return handle;
}

/** A one-use ticket an actor call consumes when the actor accepts it: an isolated or prepared
 *  graph moves to the actor, an `ArrayBuffer` is detached here and arrives without a copy. */
export function transfer<T>(value: Isolated<T>): TransferTicket<T>;
export function transfer(value: ArrayBuffer): TransferTicket<ArrayBuffer>;
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
    consume() {
      if (retained !== undefined && graph) preparedSnapshots.delete(retained);
      used = true; retained = undefined; suspended = false;
    },
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

/** @internal */
export function prepareActorMessage(
  value: any, buffers: ArrayBuffer[], consumed: Ticket[], seen = new Map<any, any>(),
  onGraphTicket?: (handle: any, ticket: Ticket) => any,
  nativePlain = false,
): any {
  if (value === null || typeof value !== "object") return value;
  // A shared graph is already an immutable message token. Walking its facade here would
  // materialize an ordinary clone (and can miss its indexed properties); the port codec owns
  // the cross-realm handle and keeps its identity and lifetime rules.
  if (typeof nativeIsShared === "function" && nativeIsShared(value)) return value;
  // Native certifies a complete graph of plain data without invoking any getter or Proxy trap.
  // It cannot contain a ticket or isolated handle. Sending the original lets postMessage's
  // shared codec make the one required clone instead of first copying the graph in TypeScript.
  // A failed certificate preserves the ticket-aware path below, including its getter behavior.
  if (nativePlain && seen.size === 0 && isolatedOp(PLAIN_MESSAGE, value) === true) return value;
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
      // One graph per message, alone: the same refusal whichever engine moves the graph.
      if (seen.has(graphInMessage)) throw dataCloneError("an actor call accepts one graph ticket");
      if (consumed.length !== 0 || buffers.length !== 0) {
        throw dataCloneError("an actor call cannot mix graph and buffer tickets");
      }
      if (localTicket === ticket && preparedSnapshots.has(ticket.value)) {
        seen.set(graphInMessage, true);
        consumed.push(ticket);
        return preparedSnapshots.get(ticket.value);
      }
      // A symbol property is only a cross-plan hint and can be forged. Moving a graph needs
      // the ticket issued by this module's private WeakMap until the engine authenticates
      // cross-plan tickets itself.
      if (onGraphTicket && localTicket === ticket) {
        seen.set(graphInMessage, true);
        return onGraphTicket(ticket.value, ticket);
      }
      throw dataCloneError("graph transfer needs ownership proof");
    }
    if (seen.has(graphInMessage)) {
      throw dataCloneError("an actor call cannot mix graph and buffer tickets");
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
    copy.length = value.length;
    seen.set(value, copy);
    for (let index = 0; index < value.length; index++) {
      if (hasOwn.call(value, index)) {
        copy[index] = prepareActorMessage(value[index], buffers, consumed, seen, onGraphTicket);
      }
    }
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

/** @internal */
export function consumeActorTickets(consumed: Ticket[]): void {
  for (const ticket of consumed) ticket.consume();
}

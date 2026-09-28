// State shared by the public actor module and its worker entry within one realm. Keeping it
// here makes their static dependency graph acyclic while ticket identities stay private.
export abstract class Actor {}
// The module graph can evaluate this internal module in more than one dynamic-import plan.
// A global symbol marks the prototype chain across those plans without sharing mutable state.
Object.defineProperty(Actor.prototype, Symbol.for("kiln.actor.base"), { value: true });

type Ticket = { value: any; consumed: boolean; graph: boolean };
const isolated = new WeakMap<object, any>();
const tickets = new WeakMap<object, Ticket>();
const isolatedMark = Symbol.for("kiln.actor.isolated");
const ticketMark = Symbol.for("kiln.actor.ticket");

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
  const value = factory();
  isolated.set(handle, value);
  // Dynamic import plans may evaluate this engine module separately in one realm. The symbol
  // lets its serializer recognize the opaque handle without exposing a cloneable value.
  Object.defineProperty(handle, isolatedMark, { value: { value } });
  return handle;
}

export function transfer<T>(value: Isolated<T> | ArrayBuffer): TransferTicket<T> {
  const sharedHandle = value !== null && typeof value === "object"
    ? (value as any)[isolatedMark] : undefined;
  const graph = isolated.has(value as object) || sharedHandle !== undefined;
  if (!graph && !(value instanceof ArrayBuffer)) {
    throw new TypeError("transfer requires an isolated handle or ArrayBuffer");
  }
  const ticket: any = { __uncloneable: () => {} };
  const entry: Ticket = {
    value: graph ? (sharedHandle === undefined ? isolated.get(value as object) : sharedHandle.value) : value,
    consumed: false,
    graph,
  };
  tickets.set(ticket, entry);
  Object.defineProperty(ticket, ticketMark, { value: entry });
  return ticket;
}

export function prepareActorMessage(value: any, buffers: ArrayBuffer[], consumed: Ticket[], seen = new Map<any, any>()): any {
  if (value === null || typeof value !== "object") return value;
  const ticket = tickets.get(value) || value[ticketMark];
  if (ticket) {
    if (ticket.consumed) throw dataCloneError("transfer ticket was already consumed");
    if (ticket.graph) throw dataCloneError("graph transfer needs ownership proof");
    buffers.push(ticket.value);
    consumed.push(ticket);
    return ticket.value;
  }
  if (isolated.has(value) || value[isolatedMark] !== undefined) {
    throw dataCloneError("isolated handle needs transfer()");
  }
  if (seen.has(value)) return seen.get(value);
  if (Array.isArray(value)) {
    const copy: any[] = [];
    seen.set(value, copy);
    for (let index = 0; index < value.length; index++) if (index in value) copy[index] = prepareActorMessage(value[index], buffers, consumed, seen);
    return copy;
  }
  // Preserve special cloneable objects and their own semantics. Tickets nested in those objects
  // cannot be found without invoking user accessors, so the serializer will reject them itself.
  if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return value;
  const copy: any = {};
  seen.set(value, copy);
  for (const key of Object.keys(value)) copy[key] = prepareActorMessage(value[key], buffers, consumed, seen);
  return copy;
}

export function consumeActorTickets(consumed: Ticket[]): void {
  for (const ticket of consumed) ticket.consumed = true;
}

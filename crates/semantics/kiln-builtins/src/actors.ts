import { Worker, MessageChannel, MessagePort } from "node:worker_threads";
import { Actor, prepareActorMessage, consumeActorTickets, type Ticket } from "kiln:actors/core";
import { actorWorkerEntry } from "kiln:actors/worker";

export { Actor, isolate, prepareTransfer, transfer } from "kiln:actors/core";
export type { Isolated, TransferTicket } from "kiln:actors/core";

// The evaluator lends these hooks only while the engine-owned module initializes. Capture
// them just as kiln:shared does; the actor module gives its own unavailable-module diagnostic.
const G = globalThis as any;
const share: (value: any) => any = G.__kilnShare;
const isShared: (value: any) => boolean = G.__kilnSharedIs;
if (typeof share !== "function" || typeof isShared !== "function") {
  throw new Error("kiln:actors requires enabled Kiln extensions");
}

const internals: any = (MessagePort as any)[Symbol.for("kiln.messagePort")];
const REGISTER = 22;
const LOOKUP = 23;
const CLOSE = 24;
const REQUEST_ID = 25;
const GRAPH_REQUEST = 30;
const GRAPH_STATUS = 31;
const GRAPH_CANCEL = 32;
const graphMarker = "__kilnActorGraphReservation";

declare const actorAddressBrand: unique symbol;
export type ActorAddress<T extends Actor = Actor> = Readonly<{ readonly [actorAddressBrand]: T }>;
type Methods<T> = { [K in keyof T & string]: T[K] extends (...args: any[]) => any ? K : never }[keyof T & string];
export interface ActorRef<T extends Actor> {
  readonly address: ActorAddress<T>;
  call<K extends Methods<T>>(
    method: K,
    args: T[K] extends (...args: infer A) => any ? A : never,
    options?: { signal?: AbortSignal },
  ): Promise<T[K] extends (...args: any[]) => infer R ? Awaited<R> : never>;
}
export interface ActorOwner<T extends Actor> {
  readonly ref: ActorRef<T>;
  stop(options?: { force?: boolean }): Promise<void>;
}

/** The actor's realm has ended, or its owner stopped it: the call can never be answered. */
export class ActorClosedError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ActorClosedError";
  }
}

/** The actor's method threw; `remoteName` is the name of what it threw, `cause` a clone of
 *  that error's cause when it could be cloned. */
export class ActorRemoteError extends Error {
  readonly remoteName: string;
  constructor(message: string, remoteName: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "ActorRemoteError";
    this.remoteName = remoteName;
  }
}

// Message-cloning and abort failures are the platform's own errors, as `structuredClone` and
// `AbortSignal` raise them.
const PlatformException: typeof DOMException = (globalThis as any).DOMException;
function dataCloneError(message: string): DOMException {
  return new PlatformException(message, "DataCloneError");
}
function remoteFailure(value: any): Error {
  const name = typeof value?.name === "string" ? value.name : "Error";
  const message = typeof value?.message === "string" ? value.message : "actor failed";
  if (name === "DataCloneError") return dataCloneError(message);
  const error = new Error(message);
  error.name = name;
  return error;
}

function senderFor(address: any): MessagePort {
  if (!isShared(address)) throw new TypeError("actor address is not a capability");
  const endpoint = internals.workerOp(LOOKUP, address);
  if (!endpoint) throw new ActorClosedError("actor address is closed or invalid");
  return internals.internalPort(endpoint, null, null, false);
}

function reference<T extends Actor>(address: ActorAddress<T>): ActorRef<T> {
  return {
    address,
    call(method: any, args: any, options?: { signal?: AbortSignal }): Promise<any> {
      return invoke(address, method, args, options, false);
    },
  };
}

// One mailbox call. `construct` is the actor's first call, made only by `spawnActor`: the
// constructor's arguments travel exactly as a method's do (ADR 0072), tickets and all.
function invoke(address: any, method: any, args: any, options: { signal?: AbortSignal } | undefined,
  construct: boolean): Promise<any> {
  if (typeof method !== "string" || !Array.isArray(args)) {
    return Promise.reject(new TypeError("actor call requires a method and argument array"));
  }
  if (options?.signal?.aborted) return Promise.reject(options.signal.reason);
  let sender: MessagePort;
  try { sender = senderFor(address); }
  catch (error) { return Promise.reject(error); }
  const id = internals.workerOp(REQUEST_ID);
  const reply = new MessageChannel();
  return new Promise((resolve, reject) => {
    let settled = false;
    let pendingGraphRequest = 0;
    let pendingGraphReservation = 0;
    let graphReplyClosed = false;
    let graphTicket: Ticket | undefined;
    let graphClaimInFlight = false;
    let graphStatusInFlight = false;
    let suspendedGraphHandle: any;
    let graphFailure: any = dataCloneError("graph transfer needs ownership proof");
    let deferredCompletion: { error: any; value: any } | undefined;
    let graphPollScheduled = false;
    let graphCancelRequested = false;
    const requestGraphCancel = (): void => {
      if (graphCancelRequested || !pendingGraphRequest) return;
      try {
        internals.workerOp(GRAPH_CANCEL, pendingGraphRequest);
        graphCancelRequested = true;
      } catch { /* A later terminal wake can retry this request. */ }
    };
    const scheduleGraphPoll = (): void => {
      if (graphPollScheduled || settled || !pendingGraphRequest) return;
      graphPollScheduled = true;
      setTimeout(pollGraph, 0);
    };
    const pollGraph = (): void => {
      graphPollScheduled = false;
      if (settled || !pendingGraphRequest || graphStatusInFlight) return;
      let status: any;
      graphStatusInFlight = true;
      try { status = internals.workerOp(GRAPH_STATUS, pendingGraphRequest); }
      catch { scheduleGraphPoll(); return; }
      finally { graphStatusInFlight = false; }
      if (status === 0) {
        if (deferredCompletion && !graphCancelRequested) {
          requestGraphCancel();
          // A synchronous cancellation may already have made status terminal. Recheck
          // once; a still-pending coordinator will wake this reply when it finishes.
          scheduleGraphPoll();
          return;
        }
        // The host wakes this reply port when the request becomes terminal. A closed
        // reply port cannot receive that wake, so only that path needs a timer retry.
        if (graphReplyClosed) scheduleGraphPoll();
        return;
      }
      if (status === 4) {
        // The host publishes 4 only after native adoption or interpreter wire
        // publication, with the sender escrow gone. A late abort cannot restore it.
        pendingGraphRequest = 0;
        pendingGraphReservation = 0;
        graphTicket?.consume();
        graphTicket = undefined;
        suspendedGraphHandle = undefined;
        if (deferredCompletion) {
          const completion = deferredCompletion;
          deferredCompletion = undefined;
          finish(completion.error, completion.value);
        }
        return;
      }
      if (status !== 1 && status !== 2 && status !== 3) {
        if (graphReplyClosed) scheduleGraphPoll();
        return;
      }
      pendingGraphRequest = 0;
      pendingGraphReservation = 0;
      try { sender.postMessage({ kind: "cancel", id }); } catch {}
      const completion = deferredCompletion;
      deferredCompletion = undefined;
      // A refused graph closes the worker's reply port after sending cloneError. That
      // close can race this status poll; it does not mean the actor realm exited.
      finish(options?.signal?.aborted ? completion?.error || graphFailure : graphFailure);
    };
    const finish = (error: any, value?: any): void => {
      if (settled) return;
      if (pendingGraphRequest || graphClaimInFlight || graphStatusInFlight) {
        // Abort, close and a fast reply all wait for the ownership transaction's terminal
        // status. Request cancellation is safe before unlink; after publication it cannot
        // restore sender ownership, so status 4 must consume the ticket instead.
        // The reply's close follows its resolve/reject message. Preserve that first
        // terminal result while the ownership status catches up.
        deferredCompletion ??= { error, value };
        if (pendingGraphRequest && !graphStatusInFlight) {
          requestGraphCancel();
          pollGraph();
        }
        return;
      }
      settled = true;
      if (graphTicket) {
        if (!error) error = dataCloneError("graph transfer needs ownership proof");
        if (suspendedGraphHandle !== undefined) {
          try { graphTicket.restore(suspendedGraphHandle); } catch {}
        }
        graphTicket = undefined;
        suspendedGraphHandle = undefined;
      }
      options?.signal?.removeEventListener("abort", abort);
      reply.port1.close();
      if (error) reject(error);
      else resolve(value);
    };
    const abort = (): void => {
      try { sender.postMessage({ kind: "cancel", id }); } catch {}
      finish(options!.signal!.reason || new PlatformException("actor call aborted", "AbortError"));
    };
    options?.signal?.addEventListener("abort", abort, { once: true });
    reply.port1.on("message", (message: any) => {
      if (settled) return;
      if (typeof message === "number") {
        if (pendingGraphRequest && message === pendingGraphReservation) pollGraph();
        return;
      }
      if (message?.kind === "grant") {
        const buffers: ArrayBuffer[] = [];
        const consumed: Ticket[] = [];
        try {
          const reservation = message.graphReservation;
          const snapshot = prepareActorMessage(args, buffers, consumed, new Map(), (_handle: any, ticket: Ticket): any => {
            // The walk has already refused a second graph or a graph beside buffers.
            if (!Number.isSafeInteger(reservation) || reservation <= 0) {
              throw dataCloneError("graph transfer needs ownership proof");
            }
            graphTicket = ticket;
            return { [graphMarker]: reservation };
          }, true);
          // Snapshot getters may synchronously abort the call. The abort handler has
          // already cancelled its reservation and retained the unsuspended ticket.
          if (settled) return;
          if (graphTicket) {
            const sent = sender.postMessage({ kind: "invoke", id, method, construct, args: snapshot,
              graphReservation: reservation });
            if (sent === undefined) throw new ActorClosedError("actor mailbox closed");
            if (settled) return;
            suspendedGraphHandle = graphTicket.suspend();
            graphClaimInFlight = true;
            try {
              const request = internals.workerOp(GRAPH_REQUEST, internals.portEndpoint(sender),
                suspendedGraphHandle, reservation, id, internals.portEndpoint(reply.port1));
              if (Number.isSafeInteger(request) && request > 0) {
                pendingGraphRequest = request;
                pendingGraphReservation = reservation;
              }
            } catch { /* A failed claim keeps the refusal and restores the ticket. */ }
            finally { graphClaimInFlight = false; }
            throw dataCloneError("graph transfer needs ownership proof");
          } else {
            const sent = sender.postMessage({ kind: "invoke", id, method, construct, args: snapshot }, buffers);
            if (sent === undefined) throw new ActorClosedError("actor mailbox closed");
            consumeActorTickets(consumed);
          }
        } catch (error) {
          if (pendingGraphRequest) {
            graphFailure = error;
            pollGraph();
          } else {
            try { sender.postMessage({ kind: "cancel", id }); } catch {}
            const completion = deferredCompletion;
            deferredCompletion = undefined;
            finish(completion?.error || error, completion?.value);
          }
        }
      } else if (message?.kind === "resolve") finish(null, message.value);
      else if (message?.kind === "reject") {
        finish(new ActorRemoteError(message.value?.message || "actor call failed", message.value?.name || "Error",
          message.value?.cause === undefined ? undefined : { cause: message.value.cause }));
      } else if (message?.kind === "cloneError") finish(dataCloneError(message.value?.message || "actor result could not be cloned"));
      else if (message?.kind === "closed") finish(new ActorClosedError("actor stopped"));
    });
    reply.port1.on("close", () => {
      graphReplyClosed = true;
      finish(new ActorClosedError("actor realm exited"));
    });
    reply.port1.start();
    try {
      if (sender.postMessage({ kind: "reserve", id, reply: reply.port2 }, [reply.port2]) === undefined) {
        finish(new ActorClosedError("actor address is closed"));
      }
    }
    catch (error) { finish(error); }
  });
}

/** A reference to the actor `address` names; any realm may hold one. Only the owner can stop it. */
export function connectActor<T extends Actor>(address: ActorAddress<T>): ActorRef<T> {
  senderFor(address);
  return reference(address);
}

/** Construct `options.export` of `options.module` (an absolute path or URL) in a realm of its own.
 *  Its constructor receives `options.args` as a call would; calls queue in a mailbox of
 *  `mailboxCapacity` (default 64). */
export async function spawnActor<T extends Actor>(options: {
  module: string | URL;
  export: string;
  args?: unknown[];
  mailboxCapacity?: number;
}): Promise<ActorOwner<T>> {
  if (!options || typeof options !== "object" || typeof options.export !== "string" || !options.export) {
    throw new TypeError("spawnActor requires { module, export }");
  }
  const module = options.module instanceof URL ? options.module.href : options.module;
  if (typeof module !== "string" || !module) throw new TypeError("actor module must be a URL or specifier");
  const absolutePath = module.startsWith("/") ||
    (module.length > 2 && module[1] === ":" && (module[2] === "\\" || module[2] === "/"));
  if (!module.startsWith("file:") && !absolutePath) {
    throw new TypeError("actor module must be an absolute path or URL; resolve relative paths with new URL(path, import.meta.url)");
  }
  const capacity = options.mailboxCapacity === undefined ? 64 : options.mailboxCapacity;
  if (!Number.isSafeInteger(capacity) || capacity <= 0) throw new RangeError("mailboxCapacity must be a positive integer");
  const args = options.args === undefined ? [] : options.args;
  if (!Array.isArray(args)) throw new TypeError("actor args must be an array");
  const address = share({}) as ActorAddress<T>;
  const ownerKey = share({});
  // The realm loads the class; its constructor's arguments are the mailbox's first call.
  const worker = new Worker(actorWorkerEntry, {
    workerData: { __kilnActorVersion: 1, module, exportName: options.export, capacity },
  });
  const endpoint = (worker as any)[Symbol.for("kiln.actor.senderEndpoint")]();
  if (!endpoint || !internals.workerOp(REGISTER, address, endpoint, ownerKey)) {
    void worker.terminate();
    throw new Error("actor mailbox registration failed");
  }
  return new Promise((resolve, reject) => {
    let ready = false;
    let exited = false;
    let stopping: Promise<void> | undefined;
    const close = (): void => {
      internals.workerOp(CLOSE, address, ownerKey);
    };
    worker.on("error", (error: any) => { close(); if (!ready) reject(error); });
    worker.on("exit", () => {
      exited = true;
      close();
      if (!ready) reject(new ActorClosedError("actor realm exited during spawn"));
    });
    const fail = (error: any): void => {
      close();
      void worker.terminate();
      reject(error);
    };
    worker.on("message", (message: any) => {
      if (message?.kind === "spawnError") {
        fail(remoteFailure(message.value));
      } else if (message?.kind === "loaded" && !ready) {
        invoke(address, "constructor", args, undefined, true).then(() => {
          ready = true;
          resolve(owner);
        }, fail);
      }
    });
    const owner: ActorOwner<T> = {
      ref: reference(address),
      stop({ force = false }: { force?: boolean } = {}): Promise<void> {
        if (stopping) return stopping;
        if (exited) return Promise.resolve();
        stopping = new Promise((done, failed) => {
          let finished = false;
          const finish = (error?: any): void => {
            if (finished) return;
            finished = true;
            close();
            if (error) failed(error);
            else done();
          };
          worker.on("message", (event: any) => {
            if (event?.kind === "stopped") void worker.terminate().then(() => finish(), finish);
          });
          worker.on("exit", () => finish());
          worker.on("error", finish);
          try { worker.postMessage({ kind: "stop", force }); }
          catch (error) { finish(error); }
        });
        return stopping;
      },
    };
  });
}

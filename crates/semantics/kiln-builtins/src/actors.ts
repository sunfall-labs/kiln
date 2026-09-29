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

function namedError(name: string, message: string): Error {
  const error = new Error(message);
  error.name = name;
  return error;
}

function senderFor(address: any): MessagePort {
  if (!isShared(address)) throw new TypeError("actor address is not a capability");
  const endpoint = internals.workerOp(LOOKUP, address);
  if (!endpoint) throw namedError("ActorClosedError", "actor address is closed or invalid");
  return internals.internalPort(endpoint, null, null, false);
}

function reference<T extends Actor>(address: ActorAddress<T>): ActorRef<T> {
  const ref: ActorRef<T> = {
    address,
    call(method: any, args: any, options?: { signal?: AbortSignal }): Promise<any> {
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
        let graphTicket: Ticket | undefined;
        let suspendedGraphHandle: any;
        let graphFailure: any = namedError("DataCloneError", "graph transfer needs ownership proof");
        let deferredCompletion: { error: any; value: any } | undefined;
        let graphPollScheduled = false;
        const scheduleGraphPoll = (): void => {
          if (graphPollScheduled || settled || !pendingGraphRequest) return;
          graphPollScheduled = true;
          setTimeout(pollGraph, 0);
        };
        const pollGraph = (): void => {
          graphPollScheduled = false;
          if (settled || !pendingGraphRequest) return;
          let status: any;
          try { status = internals.workerOp(GRAPH_STATUS, pendingGraphRequest); }
          catch { scheduleGraphPoll(); return; }
          if (status === 0) { scheduleGraphPoll(); return; }
          if (status === 4) {
            // The host publishes 4 only after the receiver owns the native root and the
            // sender's escrow entry is gone. A late abort cannot restore this ticket.
            pendingGraphRequest = 0;
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
            scheduleGraphPoll();
            return;
          }
          pendingGraphRequest = 0;
          try { sender.postMessage({ kind: "cancel", id }); } catch {}
          const completion = deferredCompletion;
          deferredCompletion = undefined;
          // A refused graph closes the worker's reply port after sending cloneError. That
          // close can race this status poll; it does not mean the actor realm exited.
          finish(options?.signal?.aborted ? completion?.error || graphFailure : graphFailure);
        };
        const finish = (error: any, value?: any): void => {
          if (settled) return;
          if (pendingGraphRequest) {
            // Abort, close and a fast reply all wait for the ownership transaction's terminal
            // status. Request cancellation is safe before unlink; after publication it cannot
            // restore sender ownership, so status 4 must consume the ticket instead.
            // The reply's close follows its resolve/reject message. Preserve that first
            // terminal result while the native ownership status catches up.
            deferredCompletion ??= { error, value };
            try { internals.workerOp(GRAPH_CANCEL, pendingGraphRequest); } catch {}
            scheduleGraphPoll();
            return;
          }
          settled = true;
          if (graphTicket) {
            if (!error) error = namedError("DataCloneError", "graph transfer needs ownership proof");
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
          finish(options!.signal!.reason || namedError("AbortError", "actor call aborted"));
        };
        options?.signal?.addEventListener("abort", abort, { once: true });
        reply.port1.on("message", (message: any) => {
          if (settled) return;
          if (message?.kind === "grant") {
            const buffers: ArrayBuffer[] = [];
            const consumed: Ticket[] = [];
            try {
              const reservation = message.graphReservation;
              const snapshot = prepareActorMessage(args, buffers, consumed, new Map(), (_handle: any, ticket: Ticket): any => {
                if (graphTicket || !Number.isSafeInteger(reservation) || reservation <= 0) {
                  throw namedError("DataCloneError", "graph transfer needs ownership proof");
                }
                graphTicket = ticket;
                return { [graphMarker]: reservation };
              });
              if (graphTicket && consumed.some((ticket) => ticket.graph)) {
                throw namedError("DataCloneError", "an actor call accepts one graph ticket");
              }
              if (graphTicket) {
                if (buffers.length !== 0) throw namedError("DataCloneError", "graph transfer needs ownership proof");
                const sent = sender.postMessage({ kind: "invoke", id, method, args: snapshot,
                  graphReservation: reservation });
                if (sent === undefined) throw namedError("ActorClosedError", "actor mailbox closed");
                if (settled) return;
                suspendedGraphHandle = graphTicket.suspend();
                try {
                  const request = internals.workerOp(GRAPH_REQUEST, internals.portEndpoint(sender),
                    suspendedGraphHandle, reservation, id, internals.portEndpoint(reply.port1));
                  if (Number.isSafeInteger(request) && request > 0) pendingGraphRequest = request;
                } catch { /* An unscheduled realm keeps the refusal and restores the ticket. */ }
                throw namedError("DataCloneError", "graph transfer needs ownership proof");
              } else {
                const sent = sender.postMessage({ kind: "invoke", id, method, args: snapshot }, buffers);
                consumeActorTickets(consumed);
                if (sent === undefined) throw namedError("ActorClosedError", "actor mailbox closed");
              }
            } catch (error) {
              if (pendingGraphRequest) {
                graphFailure = error;
                scheduleGraphPoll();
              } else {
                try { sender.postMessage({ kind: "cancel", id }); } catch {}
                finish(error);
              }
            }
          } else if (message?.kind === "resolve") finish(null, message.value);
          else if (message?.kind === "reject") {
            const error = namedError("ActorRemoteError", message.value?.message || "actor call failed");
            (error as any).remoteName = message.value?.name || "Error";
            (error as any).cause = message.value?.cause;
            finish(error);
          } else if (message?.kind === "cloneError") finish(namedError("DataCloneError", message.value?.message || "actor result could not be cloned"));
          else if (message?.kind === "closed") finish(namedError("ActorClosedError", "actor stopped"));
        });
        reply.port1.on("close", () => finish(namedError("ActorClosedError", "actor realm exited")));
        reply.port1.start();
        try {
          if (sender.postMessage({ kind: "reserve", id, reply: reply.port2 }, [reply.port2]) === undefined) {
            finish(namedError("ActorClosedError", "actor address is closed"));
          }
        }
        catch (error) { finish(error); }
      });
    },
  };
  return ref;
}

export function connectActor<T extends Actor>(address: ActorAddress<T>): ActorRef<T> {
  senderFor(address);
  return reference(address);
}

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
  const buffers: ArrayBuffer[] = [];
  const consumed: Ticket[] = [];
  const args = prepareActorMessage(options.args === undefined ? [] : options.args, buffers, consumed);
  const address = share({}) as ActorAddress<T>;
  const ownerKey = share({});
  const worker = new Worker(actorWorkerEntry, {
    workerData: { __kilnActorVersion: 1, module, exportName: options.export, args, capacity },
    transferList: buffers,
  });
  consumeActorTickets(consumed);
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
      if (!ready) reject(namedError("ActorClosedError", "actor realm exited during spawn"));
    });
    worker.on("message", (message: any) => {
      if (message?.kind === "spawnError") {
        close();
        void worker.terminate();
        const error = namedError(message.value?.name || "Error", message.value?.message || "actor constructor failed");
        reject(error);
      } else if (message?.kind === "ready" && !ready) {
        ready = true;
        resolve({
          ref: reference(address),
          stop({ force = false }: { force?: boolean } = {}): Promise<void> {
            if (stopping) return stopping;
            if (exited) return Promise.resolve();
            stopping = new Promise((done, fail) => {
              let finished = false;
              const finish = (error?: any): void => {
                if (finished) return;
                finished = true;
                close();
                if (error) fail(error);
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
        });
      }
    });
  });
}

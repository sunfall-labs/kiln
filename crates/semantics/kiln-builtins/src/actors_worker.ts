// This module is a worker entry, linked into programs that import kiln:actors. It is harmless
// when evaluated in the spawning realm. The target class is loaded only inside this new realm.
import { isMainThread, parentPort, workerData, MessagePort } from "node:worker_threads";
import { prepareActorMessage, consumeActorTickets } from "kiln:actors/core";

export const actorWorkerEntry = "kiln:actors/worker";
const BIND_RECEIVER = 33;

type Reservation = {
  id: number;
  reply: MessagePort;
  granted: boolean;
  payload: any;
};

let control: any;
let inbox: MessagePort;
let pending: Reservation[] = [];
let byId = new Map<number, Reservation>();
let capacity = 0;
let accepted = 0;
let active = 0;
let running = new Set<Reservation>();
let closing = false;
let force = false;
let completed = false;
let instance: any;

function remoteError(error: any): any {
  let cause: any = undefined;
  if (error?.cause !== undefined) {
    try { cause = structuredClone(error.cause); }
    catch { /* A remote error exposes only cloneable causes. */ }
  }
  return {
    name: typeof error?.name === "string" ? error.name : "Error",
    message: typeof error?.message === "string" ? error.message : String(error),
    cause,
  };
}

function serve(): void {
  control = parentPort;
  const init: any = workerData;
  inbox = init.mailbox;
  // Name this inbox's actual provider realm for a future opaque graph request. A failed bind
  // leaves graph requests unavailable; ordinary cloned actor messages keep working.
  const internals: any = (MessagePort as any)[Symbol.for("kiln.messagePort")];
  try { if (internals) internals.workerOp(BIND_RECEIVER, internals.portEndpoint(inbox)); }
  catch { /* An interpreter or unscheduled realm has no pair-claim coordinator. */ }
  capacity = init.capacity;

  function answer(request: Reservation, kind: string, value?: any): void {
    try {
      if (kind === "resolve") {
        const buffers: ArrayBuffer[] = [];
        const consumed: any[] = [];
        const snapshot = prepareActorMessage(value, buffers, consumed);
        request.reply.postMessage({ kind, value: snapshot }, buffers);
        consumeActorTickets(consumed);
      } else {
        request.reply.postMessage({ kind, value });
      }
    } catch (error) {
      if (kind === "resolve") {
        try { request.reply.postMessage({ kind: "cloneError", value: remoteError(error) }); }
        catch { /* The caller may have aborted after acceptance. */ }
      }
    }
    request.reply.close();
  }

  function stopped(): void {
    if (!closing || completed || active !== 0 || accepted !== 0) return;
    completed = true;
    control.postMessage({ kind: "stopped" });
    inbox.close();
  }

  function grant(): void {
    if (closing) return;
    for (const request of pending) {
      if (accepted >= capacity) break;
      if (!request.granted) {
        request.granted = true;
        accepted++;
        request.reply.postMessage({ kind: "grant" });
      }
    }
  }

  function dispatch(): void {
    while (pending.length > 0 && pending[0].granted && pending[0].payload !== undefined) {
      const request = pending.shift()!;
      byId.delete(request.id);
      accepted--;
      active++;
      running.add(request);
      grant();
      const payload = request.payload;
      let method: any;
      try {
        if (force) throw new Error("actor was stopped");
        if (typeof payload.method !== "string" || payload.method === "constructor") {
          throw new TypeError("actor method is not public");
        }
        let owner = instance;
        while (owner && owner !== Object.prototype) {
          const descriptor = Object.getOwnPropertyDescriptor(owner, payload.method);
          if (descriptor !== undefined) {
            if (typeof descriptor.value !== "function") throw new TypeError("actor method is not callable");
            method = descriptor.value;
            break;
          }
          owner = Object.getPrototypeOf(owner);
        }
        if (typeof method !== "function") throw new TypeError("actor method is not public");
        const result = method.apply(instance, payload.args);
        Promise.resolve(result).then(
          (value) => { if (running.delete(request)) { answer(request, "resolve", value); active--; stopped(); } },
          (error) => { if (running.delete(request)) { answer(request, "reject", remoteError(error)); active--; stopped(); } },
        );
      } catch (error) {
        answer(request, "reject", remoteError(error));
        running.delete(request);
        active--;
        stopped();
      }
    }
  }

  inbox.on("message", (message: any) => {
    if (!message || typeof message !== "object") return;
    if (message.kind === "reserve") {
      const request: Reservation = { id: message.id, reply: message.reply, granted: false, payload: undefined };
      if (closing) answer(request, "closed");
      else { pending.push(request); byId.set(request.id, request); grant(); }
    } else if (message.kind === "invoke") {
      const request = byId.get(message.id);
      if (request && request.granted && request.payload === undefined) {
        request.payload = message;
        dispatch();
      }
    } else if (message.kind === "cancel") {
      const request = byId.get(message.id);
      if (request && request.payload === undefined) {
        byId.delete(request.id);
        pending.splice(pending.indexOf(request), 1);
        if (request.granted) accepted--;
        request.reply.close();
        grant();
        stopped();
      }
    }
  });
  inbox.start();
  control.on("message", (message: any) => {
    if (message?.kind !== "stop") return;
    closing = true;
    force = !!message.force;
    // Graceful shutdown drains accepted reservations, including a caller that has received
    // capacity but has not yet sent its argument snapshot. Requests still waiting for capacity
    // have never been accepted and can be rejected now.
    for (let index = pending.length - 1; index >= 0; index--) {
      const request = pending[index];
      if (request.granted && !force) continue;
      pending.splice(index, 1);
      byId.delete(request.id);
      if (request.granted) accepted--;
      answer(request, "closed");
    }
    if (force) {
      for (const request of running) answer(request, "closed");
      running.clear();
      active = 0;
    }
    stopped();
  });

  void import(init.module).then((loaded: any) => {
    try {
      const ctor: any = loaded[init.exportName];
      if (typeof ctor !== "function" || !ctor.prototype || !(Symbol.for("kiln.actor.base") in ctor.prototype)) {
        throw new TypeError("actor export must be a class extending Actor");
      }
      instance = new ctor(...init.args);
      control.postMessage({ kind: "ready" });
    } catch (error) {
      control.postMessage({ kind: "spawnError", value: remoteError(error) });
      inbox.close();
    }
  }, (error: any) => {
    control.postMessage({ kind: "spawnError", value: remoteError(error) });
    inbox.close();
  });
}

const startedMark = Symbol.for("kiln.actor.worker.started");
if (!isMainThread && workerData?.__kilnActorVersion === 1 && !(globalThis as any)[startedMark]) {
  (globalThis as any)[startedMark] = true;
  serve();
}

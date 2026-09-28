// This module is a worker entry, linked into programs that import kiln:actors. It is harmless
// when evaluated in the spawning realm. The target class is loaded only inside this new realm.
import { isMainThread, parentPort, workerData, MessagePort } from "node:worker_threads";
import { prepareActorMessage, consumeActorTickets } from "kiln:actors/core";

export const actorWorkerEntry = "kiln:actors/worker";
const BIND_RECEIVER = 33;
const GRAPH_RESERVE = 34;
const GRAPH_RECEIVE = 35;
const GRAPH_RELEASE = 36;
const graphMarker = "__kilnActorGraphReservation";

type Reservation = {
  id: number;
  reply: MessagePort;
  granted: boolean;
  payload: any;
  graphReservation: number;
  graphWaiting: boolean;
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

function matchingGraphMarker(value: any, id: number): boolean {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && Object.getPrototypeOf(value) === Object.prototype
    && Object.keys(value).length === 1 && value[graphMarker] === id;
}

function countGraphMarkers(value: any, id: number, seen = new Set<any>()): number {
  if (value === null || typeof value !== "object" || seen.has(value)) return 0;
  if (matchingGraphMarker(value, id)) return 1;
  seen.add(value);
  if (Array.isArray(value)) {
    let count = 0;
    for (let i = 0; i < value.length; i++) if (i in value) count += countGraphMarkers(value[i], id, seen);
    return count;
  }
  if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return 0;
  let count = 0;
  for (const key of Object.keys(value)) count += countGraphMarkers(value[key], id, seen);
  return count;
}

function replaceGraphMarker(value: any, id: number, graph: any, seen = new Set<any>()): any {
  if (value === null || typeof value !== "object" || seen.has(value)) return value;
  if (matchingGraphMarker(value, id)) return graph;
  seen.add(value);
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) if (i in value) value[i] = replaceGraphMarker(value[i], id, graph, seen);
  } else if (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null) {
    for (const key of Object.keys(value)) value[key] = replaceGraphMarker(value[key], id, graph, seen);
  }
  return value;
}

function serve(): void {
  control = parentPort;
  const init: any = workerData;
  inbox = control;
  // Name this inbox's actual provider realm for a future opaque graph request. A failed bind
  // leaves graph requests unavailable; ordinary cloned actor messages keep working.
  const internals: any = (MessagePort as any)[Symbol.for("kiln.messagePort")];
  try { if (internals) internals.workerOp(BIND_RECEIVER, internals.portEndpoint(inbox)); }
  catch { /* An interpreter or unscheduled realm has no pair-claim coordinator. */ }
  capacity = init.capacity;

  function releaseGraphReservation(request: Reservation): void {
    if (!request.graphReservation) return;
    try { internals.workerOp(GRAPH_RELEASE, request.graphReservation); } catch {}
    request.graphReservation = 0;
  }

  function refuseGraph(request: Reservation): void {
    if (byId.get(request.id) !== request) return;
    byId.delete(request.id);
    pending.splice(pending.indexOf(request), 1);
    if (request.granted) accepted--;
    releaseGraphReservation(request);
    answer(request, "cloneError", { message: "graph transfer needs ownership proof" });
    grant();
    dispatch();
    stopped();
  }

  function receiveGraph(request: Reservation): void {
    if (byId.get(request.id) !== request || !request.graphWaiting) return;
    try {
      const answer = internals.workerOp(
        GRAPH_RECEIVE, request.graphReservation, request.id, internals.portEndpoint(request.reply),
      );
      if (!Array.isArray(answer) || answer.length !== 2) throw new Error("invalid graph receive result");
      if (answer[0] === 0) {
        setTimeout(() => receiveGraph(request), 0);
        return;
      }
      if (answer[0] !== 1) throw new Error("graph handoff refused");
      request.payload.args = replaceGraphMarker(request.payload.args, request.graphReservation, answer[1]);
      request.graphReservation = 0;
      request.graphWaiting = false;
      dispatch();
    } catch {
      refuseGraph(request);
    }
  }

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
    releaseGraphReservation(request);
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
        try {
          const reservation = internals.workerOp(
            GRAPH_RESERVE, internals.portEndpoint(inbox), request.id, internals.portEndpoint(request.reply),
          );
          if (Number.isSafeInteger(reservation) && reservation > 0) request.graphReservation = reservation;
        } catch { /* Ordinary cloned calls remain available without graph support. */ }
        request.reply.postMessage({ kind: "grant", graphReservation: request.graphReservation });
      }
    }
  }

  function dispatch(): void {
    while (pending.length > 0 && pending[0].granted && pending[0].payload !== undefined && !pending[0].graphWaiting) {
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
      if (!Number.isSafeInteger(message.id) || message.id <= 0
        || typeof message.reply?.postMessage !== "function" || typeof message.reply?.close !== "function") return;
      const request: Reservation = {
        id: message.id, reply: message.reply, granted: false, payload: undefined,
        graphReservation: 0, graphWaiting: false,
      };
      if (closing || byId.has(request.id)) answer(request, "closed");
      else { pending.push(request); byId.set(request.id, request); grant(); }
    } else if (message.kind === "invoke") {
      const request = byId.get(message.id);
      if (request && request.granted && request.payload === undefined) {
        const graphReservation = message.graphReservation;
        if (graphReservation !== undefined) {
          if (!request.graphReservation || graphReservation !== request.graphReservation
            || countGraphMarkers(message.args, graphReservation) !== 1) {
            refuseGraph(request);
            return;
          }
          request.graphWaiting = true;
        } else {
          releaseGraphReservation(request);
        }
        request.payload = message;
        if (request.graphWaiting) receiveGraph(request);
        else dispatch();
      }
    } else if (message.kind === "cancel") {
      const request = byId.get(message.id);
      // A graph placeholder is posted before the sender asks the coordinator to move its
      // candidate. If that later request refuses, cancel must also release this held invoke.
      if (request && (request.payload === undefined || request.graphWaiting)) {
        byId.delete(request.id);
        pending.splice(pending.indexOf(request), 1);
        if (request.granted) accepted--;
        request.graphWaiting = false;
        releaseGraphReservation(request);
        request.reply.close();
        grant();
        dispatch();
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

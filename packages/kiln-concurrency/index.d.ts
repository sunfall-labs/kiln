/// <reference path="./actors.d.ts" />
/** Compatibility types for the engine-owned kiln:actors extension. */
export { Actor, spawnActor, connectActor, isolate, prepareTransfer, transfer } from "kiln:actors";
export type { ActorAddress, ActorRef, ActorOwner, Isolated, TransferTicket } from "kiln:actors";

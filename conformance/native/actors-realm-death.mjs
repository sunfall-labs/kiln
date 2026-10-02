// An actor realm that ends -- an uncaught error, `process.exit` -- rejects every call still
// waiting on it with ActorClosedError (ADR 0072), including one already in flight when the realm
// died. The reply port a call waits on lives in the dead realm, so this rests on the runtime
// closing every port a worker held when it ends, as node does.
import { spawnActor } from "kiln:actors";

const target = new URL("./support/actor-mortal.mjs", import.meta.url);
const settle = (promise) => promise.then((value) => ["ok", value], (error) => ["err", error?.name]);

const crashing = await spawnActor({ module: target, export: "Mortal" });
const inFlight = settle(crashing.ref.call("slow", [300]));
console.log("crash scheduled", JSON.stringify(await settle(crashing.ref.call("crashLater", []))));
console.log("in flight", JSON.stringify(await inFlight));
console.log("after crash", JSON.stringify(await settle(crashing.ref.call("slow", [1]))));
console.log("stop", JSON.stringify(await settle(crashing.stop())));

const exiting = await spawnActor({ module: target, export: "Mortal" });
const beside = settle(exiting.ref.call("slow", [300]));
console.log("exit call", JSON.stringify(await settle(exiting.ref.call("exit", []))));
console.log("beside exit", JSON.stringify(await beside));
await exiting.stop();

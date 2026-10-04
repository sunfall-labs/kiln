// An actor's constructor arguments travel exactly as a method call's do (ADR 0072): the
// constructor is the mailbox's first call. Buffer, prepared and isolated tickets all arrive. A
// ticket is consumed once the actor accepts the call -- even if the constructor then throws, as a
// method that throws has consumed its arguments -- so only a class that never loaded leaves it.
import { spawnActor, transfer, prepareTransfer, isolate } from "kiln:actors";

const target = new URL("./support/actor-initial.mjs", import.meta.url);
const settle = (promise) => promise.then((value) => ["ok", value], (error) => ["err", error?.name, error?.remoteName]);

async function spawn(label, exportName, args, after) {
  const result = await settle(spawnActor({ module: target, export: exportName, args }));
  if (result[0] === "ok") {
    console.log(label, JSON.stringify(await result[1].ref.call("describe", [])), after ? after() : "");
    await result[1].stop();
  } else {
    console.log(label, JSON.stringify(result), after ? after() : "");
  }
}

const bytes = new Uint8Array([7, 0, 0, 0]).buffer;
await spawn("buffer ticket", "Initial", [1, transfer(bytes)], () => `left ${bytes.byteLength}`);
const prepared = transfer(prepareTransfer({ k: 1, list: [1, 2] }));
await spawn("prepared ticket", "Initial", [prepared]);
await spawn("prepared ticket again", "Initial", [prepared]);
await spawn("isolated ticket", "Initial", [transfer(isolate(() => ({ k: 2 })))]);
await spawn("isolated after candidate collection", "Initial", [transfer(isolate(() => {
  let garbage = { nested: [{ discarded: true }, { discarded: true }] };
  garbage = undefined;
  return { k: 3, live: [1, 2, 3] };
}))]);
await spawn("uncloneable", "Initial", [() => 1]);
const kept = new Uint8Array([9, 0, 0, 0]).buffer;
await spawn("constructor throws", "Throws", [transfer(kept)], () => `left ${kept.byteLength}`);
const unsent = new Uint8Array([5, 0, 0, 0]).buffer;
await spawn("missing export", "Missing", [transfer(unsent)], () => `left ${unsent.byteLength}`);
await spawn("args not an array", "Initial", "abc");
await spawn("forged brand", "Forged", []);

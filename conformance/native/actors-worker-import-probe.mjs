import { spawnActor } from "kiln:actors";
const actor = await spawnActor({
  module: new URL("./support/actor-worker-import-probe.mjs", import.meta.url),
  export: "Probe",
});
console.log(await actor.ref.call("ping", []));
await actor.stop();

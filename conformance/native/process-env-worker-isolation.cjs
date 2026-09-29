// native-env: KILN_ENV_SHARED_CHECK=original
// A sealed initial process.env layout shares only keys and attributes. Each worker owns its
// values and dictionary fallback, so writing a fresh key cannot change its parent or sibling.
const { Worker, isMainThread, parentPort, workerData } = require("node:worker_threads");
const key = "KILN_ENV_REALM_ISOLATION_Q";
const inheritedKey = "KILN_ENV_SHARED_CHECK";

if (isMainThread) {
  const before = process.env[key];
  const inheritedBefore = process.env[inheritedKey];
  process.env[inheritedKey] = "changed";
  async function sample(value) {
    const worker = new Worker(__filename, { workerData: value });
    const message = new Promise((resolve) => worker.once("message", resolve));
    const exit = new Promise((resolve) => worker.once("exit", resolve));
    return [await message, await exit];
  }
  (async () => {
    const first = await sample("first");
    const second = await sample("second");
    console.log(first[0].value, first[0].own, first[1]);
    console.log(second[0].value, second[0].own, second[1]);
    console.log(first[0].inherited, second[0].inherited);
    console.log(process.env[key] === before, process.env[inheritedKey] === "changed");
    process.env[inheritedKey] = inheritedBefore;
  })();
} else {
  const inherited = process.env[inheritedKey];
  process.env[key] = workerData;
  parentPort.postMessage({ value: process.env[key],
    own: Object.prototype.hasOwnProperty.call(process.env, key), inherited });
  parentPort.close();
}

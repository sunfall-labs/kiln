// native-env: KILN_EXTENSIONS=0
import { execFile } from "node:child_process";

const events = [];
await new Promise((resolve, reject) => {
  const child = execFile(
    "sh",
    ["-c", "printf out; printf err >&2; exit 7"],
    { encoding: "utf8" },
    (error, stdout, stderr) => {
      if (!error || error.code !== 7) return reject(new Error("missing exit error"));
      if (!child.pid || child.pid < 1) return reject(new Error("missing process id"));
      if (stdout !== "out" || stderr !== "err") return reject(new Error("wrong captured output"));
      events.push("callback");
    },
  );
  child.on("exit", (code, signal) => {
    if (code !== 7 || signal !== null) return reject(new Error("wrong exit event"));
    events.push("exit");
  });
  child.on("close", (code, signal) => {
    if (code !== 7 || signal !== null || child.exitCode !== 7) return reject(new Error("wrong close event"));
    events.push("close");
    resolve(undefined);
  });
});
console.log(events.join(","));

await new Promise((resolve, reject) => {
  const child = execFile("sh", { encoding: "utf8" }, (error, stdout, stderr) => {
    if (error || stdout !== "" || stderr !== "") return reject(new Error("options overload failed"));
  });
  child.on("close", (code, signal) => {
    if (code !== 0 || signal !== null) return reject(new Error("options overload did not exit cleanly"));
    resolve(undefined);
  });
});
console.log("options-overload");

await new Promise((resolve, reject) => {
  const child = execFile("sh", ["-c", "printf 'héllo'"], { encoding: "buffer" }, (error, stdout) => {
    if (error || !(stdout instanceof Uint8Array) || new TextDecoder().decode(stdout) !== "héllo") {
      return reject(new Error("buffer output overload failed"));
    }
  });
  child.on("close", (code, signal) => {
    if (code !== 0 || signal !== null) return reject(new Error("buffer output did not exit cleanly"));
    resolve(undefined);
  });
});
console.log("buffer-output");

const spawnFailureEvents = [];
await new Promise((resolve, reject) => {
  const child = execFile("/kiln-test-command-that-does-not-exist", [], (error, stdout, stderr) => {
    if (!error || error.code !== "ENOENT" || error.syscall !== "spawn") {
      return reject(new Error("spawn failure metadata was lost"));
    }
    if (stdout !== "" || stderr !== "") return reject(new Error("spawn failure output was not empty"));
    spawnFailureEvents.push("callback");
  });
  child.on("exit", () => reject(new Error("spawn failure emitted exit")));
  child.on("error", (error) => {
    if (!error || error.code !== "ENOENT") return reject(new Error("spawn error event was lost"));
    spawnFailureEvents.push("error");
  });
  child.on("close", (code, signal) => {
    if (code !== -2 || signal !== null) return reject(new Error("spawn failure close did not match Node"));
    spawnFailureEvents.push("close");
    resolve(undefined);
  });
});
if (spawnFailureEvents.join(",") !== "callback,error,close") {
  throw new Error("spawn failure event order differed: " + spawnFailureEvents.join(","));
}
console.log("spawn-failure");

const unsupportedKill = execFile("true");
for (const signal of [undefined, "SIGKILL", 9]) {
  let refused = false;
  try { unsupportedKill.kill(signal); } catch (error) {
    refused = String(error).includes("no child handle");
  }
  if (!refused) throw new Error("unsupported child kill did not name its missing process handle");
}
await new Promise((resolve, reject) => {
  unsupportedKill.on("error", reject);
  unsupportedKill.on("close", (code, signal) => {
    if (code !== 0 || signal !== null) return reject(new Error("kill refusal affected child completion"));
    resolve(undefined);
  });
});
console.log("kill-refusal");

// Host wake sources reach the entry module while its top-level await is pending and a guard
// timer is armed: a child process's exit, a socket's data and a file read. None of them is the
// realm's own work, so a drive that parked by jumping its virtual clock to the guard could
// hear the guard first. signal-wake-during-tla.mjs is the unreferenced case, kept apart.
import { execFile } from "node:child_process";
import { readFile } from "node:fs";
import net from "node:net";

const race = (label, start) =>
  new Promise((resolve) => {
    const guard = setTimeout(() => resolve(label + " timeout"), 3000);
    start((value) => {
      clearTimeout(guard);
      resolve(label + " " + value);
    });
  });

console.log(await race("child", (done) => {
  execFile("sh", ["-c", "sleep 0.05"], (error) => done(error ? "failed" : "exited"));
}));

const server = net.createServer((socket) => socket.end("hi"));
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
console.log(await race("socket", (done) => {
  let data = "";
  const client = net.connect(server.address().port, "127.0.0.1");
  client.on("data", (chunk) => { data += chunk; });
  client.on("end", () => done(data));
}));
server.close();

console.log(await race("fs", (done) => {
  readFile(process.argv[1], "utf8", (error, text) => done(error ? error.code : text.includes("host wake")));
}));

// 6. node at all cores: the cluster module, one worker per available core, each running
// serve.mjs. The primary distributes connections (round-robin on Linux by default).
import cluster from "node:cluster";
import os from "node:os";

if (cluster.isPrimary) {
  const n = Number(process.env.WORKERS ?? 0) || os.availableParallelism();
  for (let i = 0; i < n; i += 1) cluster.fork();
  const stop = () => {
    for (const worker of Object.values(cluster.workers)) worker.kill();
    process.exit(0);
  };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
} else {
  await import("./serve.mjs");
}

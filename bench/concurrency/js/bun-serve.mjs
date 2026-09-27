// 6. bun at all cores: one Bun.serve per available core in separate processes, all bound with
// reusePort (SO_REUSEPORT) so the kernel spreads connections -- bun's documented way to use
// every core, and the same shape as kiln's KILN_WORKERS.
import os from "node:os";

const body = "hello from kiln";
if (process.env.BUN_SERVE_CHILD) {
  Bun.serve({
    port: 18190,
    hostname: "127.0.0.1",
    reusePort: true,
    fetch: () => new Response(body, { headers: { "content-type": "text/plain" } }),
  });
  console.log("listening");
} else {
  const n = Number(process.env.WORKERS ?? 0) || os.availableParallelism();
  const children = [];
  for (let i = 0; i < n; i += 1) {
    children.push(Bun.spawn([process.execPath, import.meta.path], {
      env: { ...process.env, BUN_SERVE_CHILD: "1" },
      stdout: "ignore",
      stderr: "inherit",
    }));
  }
  const stop = () => {
    for (const child of children) child.kill();
    process.exit(0);
  };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
  console.log("listening");
}

// Load client for bench/realworld/programs/http-server.ts. Always runs on node,
// so the runtime serving the traffic is the only variable in the comparison.
//
// Sequential keep-alive requests alternating the two routes; every body is
// verified, so a fast wrong answer reads as a divergence, not a win.
import http from "node:http";

const PORT = Number(process.env.REALWORLD_PORT ?? "18091");
const N = Number(process.argv[2] ?? "2000");
const agent = new http.Agent({ keepAlive: true });

function get(path) {
  return new Promise((resolve, reject) => {
    const req = http.get(
      { host: "127.0.0.1", port: PORT, path, agent },
      (res) => {
        let body = "";
        res.on("data", (chunk) => (body += chunk));
        res.on("end", () => resolve({ status: res.statusCode, body }));
      },
    );
    req.on("error", reject);
  });
}

let wrong = 0;
const started = Date.now();
try {
  for (let i = 0; i < N; i++) {
    if (i % 2 === 0) {
      const r = await get("/hello");
      if (r.status !== 200 || r.body !== "hello from realworld") wrong++;
    } else {
      const r = await get(`/sum?a=${i}&b=1`);
      if (r.status !== 200 || r.body !== JSON.stringify({ sum: i + 1 })) wrong++;
    }
  }
} finally {
  agent.destroy();
}
const wallMs = Date.now() - started;
console.log(JSON.stringify({ requests: N, wrong, wallMs, rps: Math.round((N / wallMs) * 1000) }));

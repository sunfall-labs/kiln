// 1. Worker start cost, kiln today: kiln has no worker_threads, so the nearest equivalent is one
// KILN_WORKERS process. Built with --node; run.py sets KILN_WORKERS and reads one line per worker
// (the "message back") from the shared stdout. With `hold` the realm stays alive so run.py can
// measure the process tree's memory.
const index = process.env.KILN_WORKER_INDEX ?? "0";
console.log("ready " + index);
if (process.argv[2] === "hold") setTimeout(() => {}, 30000);

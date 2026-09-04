const pattern = /\[(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z)\]\s+([A-Z]+)\s+([a-zA-Z0-9_\-\.]+):\s+(.*)/;
const sample = "[2026-09-03T14:46:22.123Z] INFO http-server: request handled status=200 duration=12ms path=/api/v1/metrics";

let matches = 0;
for (let i = 0; i < 50000; i++) {
  const res = pattern.exec(sample);
  if (res && res[2] === "INFO") {
    matches++;
  }
}
console.log(matches);

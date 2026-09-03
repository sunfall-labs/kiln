import { readFile, stat, writeFile } from "node:fs/promises";

const filePath = ".scratch/bench-io-test.tmp";
const payload = "Hello Kiln I/O Benchmark\n".repeat(100);

async function runBenchmark() {
  for (let i = 0; i < 100; i++) {
    await writeFile(filePath, payload);
    const content = await readFile(filePath, "utf8");
    const info = await stat(filePath);
    if (info.size !== content.length) {
      throw new Error("size mismatch");
    }
  }
  console.log("fs-promises benchmark complete: 100 iterations");
}

runBenchmark();

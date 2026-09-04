import { bench } from "../harness.mjs";
import { readFile, stat, writeFile } from "node:fs/promises";

const filePath = ".scratch/bench-io-test.tmp";
const payload = "Hello Kiln I/O Benchmark\n".repeat(100);

bench("fs-promises", async () => {
  for (let i = 0; i < 100; i++) {
    await writeFile(filePath, payload);
    const content = await readFile(filePath, "utf8");
    const info = await stat(filePath);
    if (info.size !== content.length) {
      throw new Error("size mismatch");
    }
  }
  return 100;
}, { budgetMs: 1000 });

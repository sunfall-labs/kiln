import { run } from "kiln:process";
(async () => console.log((await run("printf", ["hello"])).stdout))();

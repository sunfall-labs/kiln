const { run } = require("kiln:process");

(async () => {
  const inputResult = await run("cat", [], {
    input: new Uint8Array([65, 0, 66]),
  });
  const result = await run("sh", ["-c", "printf out; printf err >&2; exit 7"]);
  const controller = new AbortController();
  const aborted = run("sleep", ["5"], { signal: controller.signal });
  controller.abort("stopped");
  let abortMessage = "unexpected";
  try {
    await aborted;
  } catch (error) {
    abortMessage = String(error);
  }
  console.log(
    inputResult.stdout,
    result.stdout,
    result.stderr,
    result.exitCode,
    result.signal,
    abortMessage,
  );
})();

// A string search value is literal even when its contents are regexp syntax. Callback
// replacements use the shared regexp walker in the native backend, so they must quote it.
const input = "${PASSWORD}";
const result = input.replace("${PASSWORD}", () => "secret");
if (result !== "secret") throw new Error(`literal callback replacement: ${result}`);
console.log(result);

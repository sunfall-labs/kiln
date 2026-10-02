// Both engines refuse the same values with the same words: `kiln-shared-graph` describes each
// refusal once, and the native provider hands its text to C rather than C inventing its own.
import { share } from "kiln:shared";

const refused = {
  function: () => 1,
  date: new Date(0),
  map: new Map(),
  symbol: Symbol("s"),
  accessor: { get value() { return 1; } },
  proxy: new Proxy({}, {}),
  instance: new (class Point {})(),
  "nested function": { list: [1, () => 2] },
  "symbol key": { [Symbol("k")]: 1 },
};
for (const [label, value] of Object.entries(refused)) {
  try {
    share(value);
    console.log(label, "shared");
  } catch (error) {
    console.log(label, "->", error.name, error.message);
  }
}

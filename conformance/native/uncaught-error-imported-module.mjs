// kiln-native: expect-exit=1
// The innermost frame is in another module, so frames name more than one file.
import { explode } from "./fixtures/uncaught-throwing-module.mjs";

function run() {
  explode("imported");
}
run();

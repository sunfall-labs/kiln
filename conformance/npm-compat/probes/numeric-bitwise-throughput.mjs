// Isolates numeric ToUint32/bitwise operations from the bcrypt implementation for paired
// native-release measurements. The fixed loop and returned checksum make skipped work visible.
import { bench } from "../../../bench/harness.mjs";

bench("npm-numeric-bitwise-loop", () => {
  let value = 0x12345678;
  for (let index = 0; index < 50_000; index += 1) {
    value ^= value << 13;
    value ^= value >>> 17;
    value ^= value << 5;
    value = (value + index) | 0;
  }
  return value >>> 0;
});

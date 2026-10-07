// Warmed, deterministic bcrypt workload for native-release measurements. A fixed valid salt
// keeps the hash output and the amount of work identical across processes and runtimes.
import bcrypt from "bcryptjs";
import { bench } from "../../../bench/harness.mjs";

const password = "kiln-npm-compat-benchmark";
const salt = "$2a$08$N9qo8uLOickgx2ZMRZoMye";
const expectedHash = "$2a$08$N9qo8uLOickgx2ZMRZoMyeoxUxJ05MRWm4bW3Giar6lxjRF5NYzvS";

bench("npm-bcryptjs-fixed-salt", () => {
  const hash = bcrypt.hashSync(password, salt);
  if (hash !== expectedHash) throw new Error(`unexpected deterministic bcrypt hash: ${hash}`);
  const matches = bcrypt.compareSync(password, hash);
  const rejects = bcrypt.compareSync("wrong password", hash) === false;
  const rounds = bcrypt.getRounds(hash);
  if (matches !== true || rejects !== true || rounds !== 8) {
    throw new Error(`bcrypt correctness check failed: match=${matches} reject=${rejects} rounds=${rounds}`);
  }
  let fingerprint = 0;
  for (let index = 7; index < hash.length; index += 1) {
    fingerprint = (Math.imul(fingerprint, 33) + hash.charCodeAt(index)) >>> 0;
  }
  return fingerprint ^ (matches ? 1 : 0) ^ (rejects ? 2 : 0) ^ rounds;
});

import "../../../counterprobe.mjs";
import "../../../expect.mjs";
import "../../../source/vitest.setup.ts";
import "../../../source/packages/effect/test/cluster/Sharding.test.ts";
import {run} from "../../../harness.mjs";
run().catch(error=>{console.error("KILN_EFFECT "+JSON.stringify({kind:"runner_error",error:String(error),stack:error.stack}));process.exitCode=1;});

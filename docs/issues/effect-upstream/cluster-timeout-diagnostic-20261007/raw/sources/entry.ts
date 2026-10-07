import "~/Work/kiln-cluster-timeout/.scratch/cluster-timeout-overlay-20261007/expect.mjs";
import "~/Work/kiln-cluster-timeout/.scratch/cluster-timeout-overlay-20261007/source/vitest.setup.ts";
import "~/Work/kiln-cluster-timeout/.scratch/cluster-timeout-overlay-20261007/ClusterWorkflowEngine.instrumented.test.ts";
import {run} from "~/Work/kiln-cluster-timeout/.scratch/cluster-timeout-overlay-20261007/harness.mjs";
run().catch(error=>{console.error("KILN_EFFECT "+JSON.stringify({kind:"runner_error",error:String(error),stack:error.stack}));process.exitCode=1;});

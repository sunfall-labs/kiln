import "~/Work/kiln/.scratch/effect-cluster-phase-diagnostic/workspace/expect.mjs";
import "~/Work/kiln/.scratch/effect-cluster-phase-diagnostic/workspace/source/vitest.setup.ts";
import "~/Work/kiln/.scratch/effect-cluster-phase-diagnostic/workspace/source/packages/effect/test/cluster/ClusterWorkflowEngine.test.ts";
import {run} from "~/Work/kiln/.scratch/effect-cluster-phase-diagnostic/workspace/harness.mjs";
run().catch(error=>{console.error("KILN_EFFECT "+JSON.stringify({kind:"runner_error",error:String(error),stack:error.stack}));process.exitCode=1;});

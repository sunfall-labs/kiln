import { makeModuleError } from "./error-stack-module.mjs";

const moduleError = makeModuleError();
const moduleStackDescriptor = Object.getOwnPropertyDescriptor(moduleError, "stack");
if (!Error.isError(moduleError) || typeof moduleError.stack !== "string" ||
    typeof moduleStackDescriptor?.get !== "function") {
  throw new Error("the shared realm lost its Error stack registration across modules");
}

console.log("error stack module registration passed");

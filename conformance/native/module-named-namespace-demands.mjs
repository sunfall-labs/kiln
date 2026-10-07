import { ns } from "./fixtures/named-namespace-demand/barrel.mjs";

if (ns.answer() !== 42) throw new Error("named namespace export lost its member");
console.log("named namespace", ns.answer());
